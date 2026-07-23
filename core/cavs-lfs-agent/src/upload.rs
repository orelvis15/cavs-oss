//! Upload one LFS object: pack the file as a single raw data track into a
//! temporary `.cavs`, then ingest it into the remote's shared
//! [`GlobalStore`] (chunk-level dedup against every object ever pushed).
//!
//! Publication is session-batched (Xet-style finalize): uploads only queue
//! the oid, and the static export tree that downloads read is refreshed once
//! per push, at terminate (see [`crate::store_sync::WriteSession::finalize`]).
//! An interrupted push publishes nothing — the next push re-ingests and
//! repairs, which the idempotent re-push path below also covers.

use crate::protocol::{Progress, ProtoOut};
use crate::store_sync::WriteSession;
use anyhow::{bail, Context, Result};
use cavs_chunker::ChunkMode;
use cavs_format::{
    ingest_into_store, IngestStats, Reader, SegmentRecord, TrackKind, TrackRecord, Writer,
    SEGMENT_FLAG_RANDOM_ACCESS,
};
use std::path::Path;

/// Upload configuration fixed for the whole session.
#[derive(Debug, Clone)]
pub struct UploadCfg {
    /// `--profile auto`: pick the chunking profile per file by size.
    pub auto: bool,
    pub mode: ChunkMode,
    pub profile_label: &'static str,
    pub compress: bool,
    pub zstd_level: i32,
    /// Consider the BG4 byte-grouping pretransform per chunk (numeric
    /// payloads); safe here because the agent's export tree is only read by
    /// BG4-aware consumers (cavs-fetch).
    pub bg4: bool,
    pub sign_key: Option<[u8; 32]>,
}

/// Size-tiered automatic profile selection, tuned from the committed
/// benchmark sweep (bench/RESULTS.md): small chunks win on small and
/// compressible files (update download −71% at 64 MiB compressible),
/// fastcdc-64k wins on large incompressible blobs, and larger chunks bound
/// per-asset metadata (manifest/chunk-map scale with chunk count) on huge
/// files. Deliberately a pure function of size: the agent sees each LFS
/// object in isolation, and a stable choice keeps chunk boundaries — and
/// therefore cross-version dedup — intact as a file evolves. A file whose
/// size crosses a tier loses dedup for that one transition.
pub fn auto_profile(size: u64) -> &'static str {
    const MIB: u64 = 1024 * 1024;
    if size < 128 * MIB {
        // Strictly better on the 64 MiB compressible scenario (storage −20%,
        // update download −71% vs 64k) and better download at 104 MiB too
        // (−19%) for +3.5% storage — update efficiency is the product goal.
        "fastcdc-16k"
    } else if size < 512 * MIB {
        "fastcdc-64k"
    } else {
        "fastcdc-128k"
    }
}

/// Same labels/modes as cavs-cli's `ChunkProfile` and the SDK's
/// `parse_profile` — chunk boundaries are part of a profile's identity, so
/// the tables must stay in lockstep. (`auto` is handled before this table:
/// see [`auto_profile`].)
pub fn parse_profile(label: &str) -> Result<(ChunkMode, &'static str)> {
    let cdc = |min: usize, avg: usize, max: usize, norm: u8| ChunkMode::Cdc {
        min: min * 1024,
        avg: avg * 1024,
        max: max * 1024,
        norm,
    };
    Ok(match label {
        "fastcdc-64k" => (cdc(16, 64, 256, cavs_chunker::NORM_DEFAULT), "fastcdc-64k"),
        "fastcdc-16k" => (cdc(4, 16, 64, cavs_chunker::NORM_TIGHT), "fastcdc-16k"),
        "fastcdc-32k" => (cdc(8, 32, 128, cavs_chunker::NORM_TIGHT), "fastcdc-32k"),
        "fastcdc-128k" => (
            cdc(32, 128, 512, cavs_chunker::NORM_DEFAULT),
            "fastcdc-128k",
        ),
        "fastcdc-256k" => (
            cdc(64, 256, 1024, cavs_chunker::NORM_DEFAULT),
            "fastcdc-256k",
        ),
        "fastcdc-64k-n3" => (cdc(16, 64, 256, cavs_chunker::NORM_TIGHT), "fastcdc-64k-n3"),
        "fastcdc-128k-n3" => (
            cdc(32, 128, 512, cavs_chunker::NORM_TIGHT),
            "fastcdc-128k-n3",
        ),
        "fixed-256k" => (ChunkMode::Fixed { size: 256 * 1024 }, "fixed-256k"),
        "fixed-512k" => (ChunkMode::Fixed { size: 512 * 1024 }, "fixed-512k"),
        "fixed-1m" => (ChunkMode::Fixed { size: 1024 * 1024 }, "fixed-1m"),
        other => bail!("unknown profile '{other}'"),
    })
}

/// `none` or `zstd-<1..22>` (same grammar as the SDK/CLI).
pub fn parse_compression(s: &str) -> Result<(bool, i32)> {
    if s == "none" {
        return Ok((false, 3));
    }
    if let Some(level) = s.strip_prefix("zstd-") {
        if let Ok(level) = level.parse::<i32>() {
            if (1..=22).contains(&level) {
                return Ok((true, level));
            }
        }
    }
    bail!("unknown compression '{s}' (expected zstd-<1..22> or none)")
}

/// Load a 64-hex Ed25519 secret key file (the format `cavs keygen` writes).
pub fn load_sign_key(path: &Path) -> Result<[u8; 32]> {
    let hex = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read sign key {}", path.display()))?;
    let hex = hex.trim();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("sign key must be 64 hex chars (32 bytes)");
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
    }
    Ok(key)
}

/// Push `src` (whose content sha256 is `oid`) into the directory remote.
/// `write` is the session-scoped lock + store; the asset is ingested here
/// and exported at session finalize.
pub fn handle(
    write: &mut WriteSession,
    oid: &str,
    src: &Path,
    size: u64,
    cfg: &UploadCfg,
    out: &ProtoOut,
) -> Result<IngestStats> {
    let tree = write.tree.clone();
    // Idempotent re-push: the object is already published. Refresh its
    // export at finalize only if its manifest is missing from the tree
    // (e.g. a crash between a previous session's commit and export).
    if write.store.has_asset(oid) {
        if !tree
            .join("assets")
            .join(oid)
            .join("manifest.json")
            .is_file()
        {
            write.pending_exports.push(oid.to_string());
        }
        eprintln!(
            "[lfs-agent] upload {}: already at remote, skipping",
            &oid[..12.min(oid.len())]
        );
        return Ok(IngestStats::default());
    }

    // Resolve `--profile auto` per file, by size (see auto_profile).
    let mut eff = cfg.clone();
    if cfg.auto {
        let picked = auto_profile(size);
        let (mode, label) = parse_profile(picked)?;
        eff.mode = mode;
        eff.profile_label = label;
        eprintln!(
            "[lfs-agent] upload {}: auto profile -> {label} ({size} bytes)",
            &oid[..12.min(oid.len())]
        );
    }

    // 1. Pack the blob as a `.cavs` with one raw data track named after the
    //    oid. `sha256:<oid> = <oid>` lets cavs-fetch verify the LFS oid on
    //    every future download.
    let tmp = tempfile::Builder::new()
        .prefix(oid)
        .suffix(".cavs")
        .tempfile_in(&tree)?;
    pack_blob(src, oid, tmp.path(), &eff)?;
    // Coarse milestones; bytesSoFar is monotonic and the bytesSinceLast
    // deltas sum exactly to `size` (git-lfs accounts transferred bytes).
    let m1 = size / 2;
    out.send(&Progress::new(oid, m1, m1));

    // 2. Ingest into the shared store: only chunks new to the store are
    //    written (dedup against every version/object ever pushed). The
    //    publish is batched — the ingest pack aggregates across the whole
    //    push and the ledger is committed once, at session finalize.
    let mut reader = Reader::open(tmp.path())?;
    let stats = ingest_into_store(&mut reader, &mut write.store, oid)?;
    drop(reader);
    let _ = tmp.close();
    eprintln!(
        "[lfs-agent] upload {}: {} chunks, {} new ({} bytes stored)",
        &oid[..12.min(oid.len())],
        stats.chunks,
        stats.new_chunks,
        stats.new_bytes
    );
    out.send(&Progress::new(oid, size, size - m1));

    // 3. Queue the export: the static tree is refreshed once per push, at
    //    terminate — O(new data) pack copies for the whole session instead
    //    of per-object re-copies of the still-growing ingest pack.
    write.pending_exports.push(oid.to_string());
    Ok(stats)
}

/// Pack a single file as one raw data track into `dst`.
fn pack_blob(src: &Path, oid: &str, dst: &Path, cfg: &UploadCfg) -> Result<()> {
    let file = std::fs::File::open(src)
        .with_context(|| format!("cannot open LFS object {}", src.display()))?;
    // Safety: git-lfs hands us a private temp copy; nobody mutates it while
    // we read.
    let data = unsafe { memmap2::Mmap::map(&file)? };

    // Deterministic asset uuid derived from the oid: re-packing the same
    // content anywhere yields the same identity.
    let mut uuid = [0u8; 16];
    for (i, byte) in uuid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&oid[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("oid is not hex: {oid}"))?;
    }

    let mut w = Writer::create(dst, uuid, 1000, cfg.compress)
        .with_context(|| format!("cannot create {}", dst.display()))?;
    w.set_zstd_level(cfg.zstd_level);
    w.set_bg4(cfg.bg4);
    if let Some(secret) = &cfg.sign_key {
        w.sign_with(secret);
    }
    w.set_meta(
        "packer",
        concat!("cavs-lfs-agent ", env!("CARGO_PKG_VERSION")),
    );
    w.set_meta("payload", "raw");
    w.set_meta(&format!("sha256:{oid}"), oid);
    w.set_meta(&format!("profile:{oid}"), cfg.profile_label);

    let ranges = cavs_chunker::split(&data, cfg.mode);
    let chunks = w.add_chunks_parallel(&data, &ranges)?;
    w.add_track(TrackRecord {
        track_id: 1,
        kind: TrackKind::Data,
        flags: 0,
        codec: "raw".to_string(),
        name: oid.to_string(),
        timescale: 1000,
        init_chunks: Vec::new(),
    })?;
    w.add_segment(SegmentRecord {
        segment_id: 0,
        track_id: 1,
        pts_start: 0,
        duration: 0,
        flags: SEGMENT_FLAG_RANDOM_ACCESS,
        chunks,
    })?;
    w.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_profile_tiers() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(auto_profile(0), "fastcdc-16k");
        assert_eq!(auto_profile(360 * 1024), "fastcdc-16k");
        assert_eq!(auto_profile(64 * MIB), "fastcdc-16k");
        assert_eq!(auto_profile(127 * MIB), "fastcdc-16k");
        assert_eq!(auto_profile(128 * MIB), "fastcdc-64k");
        assert_eq!(auto_profile(511 * MIB), "fastcdc-64k");
        assert_eq!(auto_profile(512 * MIB), "fastcdc-128k");
        assert_eq!(auto_profile(4096 * MIB), "fastcdc-128k");
    }

    #[test]
    fn auto_tiers_are_parseable() {
        // Every label auto_profile can return must exist in the table.
        for size in [0, 64 * 1024 * 1024, 512 * 1024 * 1024] {
            parse_profile(auto_profile(size)).unwrap();
        }
    }

    #[test]
    fn explicit_auto_label_is_not_in_the_table() {
        // `auto` is resolved before parse_profile; the table must reject it
        // so nothing accidentally treats it as a concrete profile.
        assert!(parse_profile("auto").is_err());
    }
}
