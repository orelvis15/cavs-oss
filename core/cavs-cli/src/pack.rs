//! Packing (conversion into .cavs).
//!
//! Video mode: each input is segmented with ffmpeg into CMAF/fMP4
//! (init.mp4 + seg_%05d.m4s + media.m3u8), and those artifacts are chunked,
//! deduplicated and stored. Byte-identical content across segments and across
//! inputs (shared intros, repeated stills, same episode at two cuts...) is
//! stored exactly once.
//!
//! Raw mode: arbitrary files are chunked with FastCDC and stored as data
//! tracks; unpacking reproduces them byte-for-byte.

use crate::{ffmpeg, report, ChunkModeArg};
use anyhow::{bail, Context, Result};
use cavs_chunker::ChunkMode;
use cavs_format::{SegmentRecord, TrackKind, TrackRecord, Writer, SEGMENT_FLAG_RANDOM_ACCESS};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Track-id namespace: playlist/data companion tracks of video track `n`
/// live at `PLAYLIST_TRACK_BASE + n`.
pub const PLAYLIST_TRACK_BASE: u32 = 1000;

pub struct PackOptions {
    pub segment_time: f64,
    pub mode: Option<ChunkModeArg>,
    pub chunk_size: Option<usize>,
    pub compress: bool,
    pub zstd_level: i32,
    pub force_transcode: bool,
    pub sign_key: Option<PathBuf>,
}

/// Load an Ed25519 secret key (hex file from `cavs keygen`) and attach it.
fn apply_signing(w: &mut Writer, opts: &PackOptions) -> Result<()> {
    let Some(path) = &opts.sign_key else {
        return Ok(());
    };
    let hex = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read signing key {}", path.display()))?;
    let hex = hex.trim();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("{} is not a 64-hex-char Ed25519 secret key", path.display());
    }
    let mut secret = [0u8; 32];
    for (i, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    w.sign_with(&secret);
    eprintln!("[pack] content will be signed (Ed25519)");
    Ok(())
}

impl PackOptions {
    fn media_mode(&self) -> ChunkMode {
        match self.mode {
            Some(m) => m.to_mode(self.chunk_size),
            None => ChunkMode::media_default(),
        }
    }

    fn asset_mode(&self) -> ChunkMode {
        match self.mode {
            Some(m) => m.to_mode(self.chunk_size),
            None => ChunkMode::asset_default(),
        }
    }
}

/// Add `data` as chunks and return the chunk indices.
fn add_chunked(w: &mut Writer, data: &[u8], mode: ChunkMode) -> Result<Vec<u32>> {
    let mut idxs = Vec::new();
    for range in cavs_chunker::split(data, mode) {
        idxs.push(w.add_chunk(&data[range])?);
    }
    Ok(idxs)
}

/// Derive a unique, filesystem-safe logical name from an input path.
fn unique_stem(path: &Path, used: &mut HashSet<String>) -> String {
    let base = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "input".to_string())
        .replace(['/', '\\'], "_");
    let mut name = base.clone();
    let mut n = 1;
    while !used.insert(name.clone()) {
        n += 1;
        name = format!("{base}-{n}");
    }
    name
}

pub fn pack_video(inputs: &[PathBuf], output: &Path, opts: &PackOptions) -> Result<()> {
    ffmpeg::require("ffmpeg")?;

    let uuid = *uuid::Uuid::new_v4().as_bytes();
    let mut w = Writer::create(output, uuid, 1000, opts.compress)
        .with_context(|| format!("cannot create {}", output.display()))?;
    w.set_zstd_level(opts.zstd_level);
    apply_signing(&mut w, opts)?;
    w.set_meta("packer", concat!("cavs-cli ", env!("CARGO_PKG_VERSION")));
    w.set_meta("payload", "cmaf-hls");
    w.set_meta("segment_time", &format!("{}", opts.segment_time));

    let mut used_names = HashSet::new();
    let mut segment_id = 0u64;

    for (i, input) in inputs.iter().enumerate() {
        let track_id = i as u32 + 1;
        let stem = unique_stem(input, &mut used_names);
        eprintln!("[pack] segmenting {} with ffmpeg...", input.display());

        let workdir = tempfile::tempdir().context("cannot create temp dir")?;
        let copied =
            ffmpeg::segment_to_cmaf(input, workdir.path(), opts.segment_time, opts.force_transcode)?;
        eprintln!(
            "[pack]   mode: {}",
            if copied { "stream copy" } else { "transcode h264+aac" }
        );

        // Init segment: pinned into the global dictionary (bootstrap payload).
        let init_bytes = std::fs::read(workdir.path().join("init.mp4"))
            .context("ffmpeg did not produce init.mp4")?;
        let init_chunks = add_chunked(&mut w, &init_bytes, opts.media_mode())?;
        for &c in &init_chunks {
            w.pin_dict(c)?;
        }

        let playlist_text = std::fs::read_to_string(workdir.path().join("media.m3u8"))
            .context("ffmpeg did not produce media.m3u8")?;
        let playlist_segments = ffmpeg::parse_playlist(&playlist_text);
        if playlist_segments.is_empty() {
            bail!("playlist has no media segments");
        }

        w.add_track(TrackRecord {
            track_id,
            kind: TrackKind::Video,
            flags: 0,
            codec: ffmpeg::probe_codecs(input),
            name: stem.clone(),
            timescale: 1000,
            init_chunks,
        })?;

        // Every fMP4 segment starts at a random-access point (keyframe
        // bundle boundary), so each CAVS segment is self-sufficient given
        // the init chunks.
        let mut pts_ms = 0u64;
        for (uri, dur_secs) in &playlist_segments {
            let seg_bytes = std::fs::read(workdir.path().join(uri))
                .with_context(|| format!("missing media segment {uri}"))?;
            let chunks = add_chunked(&mut w, &seg_bytes, opts.media_mode())?;
            let dur_ms = (dur_secs * 1000.0).round() as u32;
            w.add_segment(SegmentRecord {
                segment_id,
                track_id,
                pts_start: pts_ms,
                duration: dur_ms,
                flags: SEGMENT_FLAG_RANDOM_ACCESS,
                chunks,
            })?;
            segment_id += 1;
            pts_ms += dur_ms as u64;
        }

        // The playlist itself, as a companion data track, so unpacking can
        // emit a directly playable HLS layout.
        let playlist_chunks = add_chunked(&mut w, playlist_text.as_bytes(), opts.asset_mode())?;
        w.add_track(TrackRecord {
            track_id: PLAYLIST_TRACK_BASE + track_id,
            kind: TrackKind::Data,
            flags: 0,
            codec: "m3u8".to_string(),
            name: format!("{stem}/media.m3u8"),
            timescale: 1000,
            init_chunks: Vec::new(),
        })?;
        w.add_segment(SegmentRecord {
            segment_id,
            track_id: PLAYLIST_TRACK_BASE + track_id,
            pts_start: 0,
            duration: 0,
            flags: SEGMENT_FLAG_RANDOM_ACCESS,
            chunks: playlist_chunks,
        })?;
        segment_id += 1;

        eprintln!(
            "[pack]   {} media segments, total {:.1}s",
            playlist_segments.len(),
            pts_ms as f64 / 1000.0
        );
    }

    let stats = w.finish()?;
    report::print_pack_stats(output, &stats);
    Ok(())
}

pub fn pack_raw(inputs: &[PathBuf], output: &Path, opts: &PackOptions) -> Result<()> {
    let uuid = *uuid::Uuid::new_v4().as_bytes();
    let mut w = Writer::create(output, uuid, 1000, opts.compress)
        .with_context(|| format!("cannot create {}", output.display()))?;
    w.set_zstd_level(opts.zstd_level);
    apply_signing(&mut w, opts)?;
    w.set_meta("packer", concat!("cavs-cli ", env!("CARGO_PKG_VERSION")));
    w.set_meta("payload", "raw");

    let mut used_names = HashSet::new();
    for (i, input) in inputs.iter().enumerate() {
        let name = {
            let file_name = input
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("file-{i}"));
            let mut candidate = file_name.clone();
            let mut n = 1;
            while !used_names.insert(candidate.clone()) {
                n += 1;
                candidate = format!("{n}-{file_name}");
            }
            candidate
        };
        let data = std::fs::read(input)
            .with_context(|| format!("cannot read {}", input.display()))?;
        eprintln!(
            "[pack] {} ({} bytes) as raw asset track",
            input.display(),
            data.len()
        );
        // Per-file SHA-256 in meta: lets thin clients (e.g. the Godot
        // GDScript runtime, which has no BLAKE3) verify reconstruction
        // end-to-end with their built-in hasher.
        {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(&data);
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            w.set_meta(&format!("sha256:{name}"), &hex);
        }
        let chunks = add_chunked(&mut w, &data, opts.asset_mode())?;
        let track_id = i as u32 + 1;
        w.add_track(TrackRecord {
            track_id,
            kind: TrackKind::Data,
            flags: 0,
            codec: "raw".to_string(),
            name,
            timescale: 1000,
            init_chunks: Vec::new(),
        })?;
        w.add_segment(SegmentRecord {
            segment_id: i as u64,
            track_id,
            pts_start: 0,
            duration: 0,
            flags: SEGMENT_FLAG_RANDOM_ACCESS,
            chunks,
        })?;
    }

    let stats = w.finish()?;
    report::print_pack_stats(output, &stats);
    Ok(())
}
