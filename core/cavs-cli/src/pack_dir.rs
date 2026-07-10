//! `cavs pack-dir` (stable since v0.7.0) — package a directory tree as a
//! deduplicated container: one data track per file (relative path as the
//! logical name), plus meta records for empty directories, symlinks and
//! executable bits. Clients apply directory assets with per-file no-op
//! detection and staged, journaled installs.
//!
//! Path rules: entries travel as UTF-8 forward-slash relative paths;
//! absolute paths and `..` traversal are rejected. Ignore rules come from
//! `--ignore` globs and a `.cavsignore` file at the tree root.
//!
//! Platform notes:
//! - Unix permissions are reduced to one executable bit, best-effort on
//!   Windows.
//! - Symlinks are recorded and recreated on Unix; skipped elsewhere.
//! - Hardlinks are not detected (each file packs independently; dedup
//!   makes the cost negligible).

use crate::ignore::IgnoreRules;
use crate::profile::ChunkProfile;
use crate::report;
use anyhow::{bail, Context, Result};
use cavs_chunker::ChunkMode;
use cavs_format::{SegmentRecord, TrackKind, TrackRecord, Writer, SEGMENT_FLAG_RANDOM_ACCESS};
use std::path::{Path, PathBuf};

pub struct PackDirOptions {
    /// Fixed profile label; `auto`/absent uses the benchmark-validated
    /// update default (fastcdc-64k).
    pub profile: Option<String>,
    pub compress: bool,
    pub zstd_level: i32,
    pub sign_key: Option<PathBuf>,
    /// `--ignore` glob patterns (merged with the root's `.cavsignore`).
    pub ignore: Vec<String>,
}

pub fn pack_dir(input: &Path, output: &Path, opts: &PackDirOptions) -> Result<()> {
    if !input.is_dir() {
        bail!("{} is not a directory", input.display());
    }
    let (mode, label) = match opts.profile.as_deref() {
        None | Some("auto") => (ChunkProfile::FastCdc64K.to_mode(), "fastcdc-64k"),
        Some(other) => {
            let p = ChunkProfile::parse(other)?;
            (p.to_mode(), p.label())
        }
    };

    let uuid = *uuid::Uuid::new_v4().as_bytes();
    let mut w = Writer::create(output, uuid, 1000, opts.compress)
        .with_context(|| format!("cannot create {}", output.display()))?;
    w.set_zstd_level(opts.zstd_level);
    if let Some(key) = &opts.sign_key {
        crate::pack::sign_writer(&mut w, key)?;
    }
    w.set_meta("packer", concat!("cavs-cli ", env!("CARGO_PKG_VERSION")));
    w.set_meta("payload", "directory");

    let rules = IgnoreRules::load(input, &opts.ignore)?;
    let entries = walk_sorted(input)?;
    let mut track_id = 0u32;
    let mut segment_id = 0u64;
    let mut files = 0u64;
    let mut ignored = 0u64;
    let mut total_bytes = 0u64;
    for rel in entries {
        let full = input.join(&rel);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !cavs_plan::path_is_safe(&rel_str) {
            bail!(
                "{}",
                cavs_proto::errors::ErrorCode::PathTraversal
                    .msg(format!("unsafe path in tree: {rel_str}"))
            );
        }
        let meta = std::fs::symlink_metadata(&full)?;
        if rules.matches(&rel_str, meta.is_dir() && !meta.file_type().is_symlink()) {
            ignored += 1;
            continue;
        }
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full)?;
            w.set_meta(&format!("symlink:{rel_str}"), &target.to_string_lossy());
            continue;
        }
        if meta.is_dir() {
            w.set_meta(&format!("dir:{rel_str}"), "1");
            continue;
        }

        let data =
            std::fs::read(&full).with_context(|| format!("cannot read {}", full.display()))?;
        files += 1;
        total_bytes += data.len() as u64;
        {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(&data);
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            w.set_meta(&format!("sha256:{rel_str}"), &hex);
        }
        w.set_meta(&format!("profile:{rel_str}"), label);
        if is_executable(&meta) {
            w.set_meta(&format!("exec:{rel_str}"), "1");
        }

        let chunks = add_chunked(&mut w, &data, mode)?;
        track_id += 1;
        w.add_track(TrackRecord {
            track_id,
            kind: TrackKind::Data,
            flags: 0,
            codec: "raw".to_string(),
            name: rel_str,
            timescale: 1000,
            init_chunks: Vec::new(),
        })?;
        w.add_segment(SegmentRecord {
            segment_id,
            track_id,
            pts_start: 0,
            duration: 0,
            flags: SEGMENT_FLAG_RANDOM_ACCESS,
            chunks,
        })?;
        segment_id += 1;
    }

    if files == 0 {
        bail!("{} contains no files", input.display());
    }
    eprintln!(
        "[pack-dir] {files} files, {total_bytes} bytes, profile {label}{}",
        if ignored > 0 {
            format!(" ({ignored} entries ignored)")
        } else {
            String::new()
        }
    );
    let stats = w.finish()?;
    report::print_pack_stats(output, &stats);
    Ok(())
}

fn add_chunked(w: &mut Writer, data: &[u8], mode: ChunkMode) -> Result<Vec<u32>> {
    let ranges = cavs_chunker::split(data, mode);
    Ok(w.add_chunks_parallel(data, &ranges)?)
}

/// Deterministic walk: every path under `root`, sorted, symlinks not
/// followed.
fn walk_sorted(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(|e| e.path())
            .collect();
        children.sort();
        for child in children {
            let meta = std::fs::symlink_metadata(&child)?;
            out.push(child.strip_prefix(root).unwrap().to_path_buf());
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}
