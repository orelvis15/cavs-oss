//! Unpacking (.cavs -> playable media) and playback.

use crate::ffmpeg;
use anyhow::{bail, Context, Result};
use cavs_format::{Reader, TrackKind};
use std::fs;
use std::path::{Path, PathBuf};

/// Reconstruct all tracks into `output`.
///
/// For each video track `stem`:
/// - `output/stem/init.mp4`, `output/stem/seg_%05d.m4s`, `output/stem/media.m3u8`
///   (a directly playable HLS/CMAF layout), and
/// - `output/stem.mp4`: init + segments concatenated, a valid progressive
///   fragmented MP4 (if `combined_mp4`).
///
/// Data tracks are written to their logical name (raw mode reproduces the
/// original files byte-for-byte).
///
/// Returns the list of "primary" playable files (combined mp4s, or raw files).
pub fn unpack(input: &Path, output: &Path, combined_mp4: bool) -> Result<Vec<PathBuf>> {
    let mut r = Reader::open(input)
        .with_context(|| format!("cannot open {}", input.display()))?;
    fs::create_dir_all(output)?;

    let tracks = r.tracks().to_vec();
    let mut primaries = Vec::new();

    for track in &tracks {
        match track.kind {
            TrackKind::Video | TrackKind::Audio => {
                let dir = output.join(&track.name);
                fs::create_dir_all(&dir)?;

                let init = r.track_init_bytes(track.track_id)?;
                fs::write(dir.join("init.mp4"), &init)?;

                let segs: Vec<_> = r
                    .segments_for_track(track.track_id)
                    .into_iter()
                    .cloned()
                    .collect();
                let mut combined = if combined_mp4 { init.clone() } else { Vec::new() };
                for (ordinal, seg) in segs.iter().enumerate() {
                    let bytes = r.segment_bytes(seg)?;
                    fs::write(dir.join(format!("seg_{ordinal:05}.m4s")), &bytes)?;
                    if combined_mp4 {
                        combined.extend_from_slice(&bytes);
                    }
                }
                eprintln!(
                    "[unpack] track {} ({}): {} segments -> {}/",
                    track.track_id,
                    track.name,
                    segs.len(),
                    dir.display()
                );
                if combined_mp4 {
                    let mp4_path = output.join(format!("{}.mp4", track.name));
                    fs::write(&mp4_path, &combined)?;
                    eprintln!("[unpack] combined mp4 -> {}", mp4_path.display());
                    primaries.push(mp4_path);
                }
            }
            TrackKind::Subtitle | TrackKind::Data => {
                // Logical name may contain a relative sub-path (e.g.
                // "stem/media.m3u8"); sanitize against traversal.
                if track.name.contains("..") || track.name.starts_with('/') {
                    bail!("track {} has an unsafe name: {}", track.track_id, track.name);
                }
                let path = output.join(&track.name);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let segs: Vec<_> = r
                    .segments_for_track(track.track_id)
                    .into_iter()
                    .cloned()
                    .collect();
                let mut bytes = Vec::new();
                for seg in &segs {
                    bytes.extend_from_slice(&r.segment_bytes(seg)?);
                }
                fs::write(&path, &bytes)?;
                eprintln!(
                    "[unpack] data track {} -> {} ({} bytes)",
                    track.track_id,
                    path.display(),
                    bytes.len()
                );
                if track.codec == "raw" {
                    primaries.push(path);
                }
            }
        }
    }

    Ok(primaries)
}

/// Reconstruct to a temp dir and play the first playable output with ffplay.
pub fn play(input: &Path) -> Result<()> {
    let tmp = tempfile::tempdir().context("cannot create temp dir")?;
    let primaries = unpack(input, tmp.path(), true)?;
    let Some(target) = primaries.first() else {
        bail!("no playable track found in {}", input.display());
    };
    eprintln!("[play] launching ffplay on {}", target.display());
    ffmpeg::play_file(target)
}
