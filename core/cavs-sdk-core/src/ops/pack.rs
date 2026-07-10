//! `packDirectory` — package a directory tree as a deduplicated `.cavs`
//! container. Same output shape as `cavs pack-dir`: one data track per
//! file, meta records for empty dirs / symlinks / exec bits, SHA-256 per
//! file, `.cavsignore` support.

use crate::error::{Result, SdkError};
use crate::fsutil::{is_executable, walk_sorted, IgnoreRules};
use crate::progress::OpCtx;
use cavs_chunker::ChunkMode;
use cavs_format::{SegmentRecord, TrackKind, TrackRecord, Writer, SEGMENT_FLAG_RANDOM_ACCESS};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackDirectoryRequest {
    input_dir: PathBuf,
    output_cavs: PathBuf,
    /// fixed-256k | fixed-512k | fixed-1m | fastcdc-16k | fastcdc-32k |
    /// fastcdc-64k | fastcdc-128k | fastcdc-256k | fastcdc-64k-n3 |
    /// fastcdc-128k-n3 | auto (benchmark-validated default: fastcdc-64k).
    #[serde(default = "default_profile")]
    profile: String,
    /// "zstd-<level>" or "none".
    #[serde(default = "default_compression")]
    compression: String,
    /// Path to a 64-hex-char Ed25519 secret key; signs the container.
    #[serde(default)]
    sign_key_path: Option<PathBuf>,
    /// Extra ignore globs, merged with the root's `.cavsignore`.
    #[serde(default)]
    ignore: Vec<String>,
}

fn default_profile() -> String {
    "auto".to_string()
}

fn default_compression() -> String {
    "zstd-3".to_string()
}

/// Same labels/modes as cavs-cli's `ChunkProfile`.
fn parse_profile(label: &str) -> Result<(ChunkMode, &'static str)> {
    Ok(match label {
        "auto" | "fastcdc-64k" => (
            ChunkMode::Cdc {
                min: 16 * 1024,
                avg: 64 * 1024,
                max: 256 * 1024,
                norm: cavs_chunker::NORM_DEFAULT,
            },
            "fastcdc-64k",
        ),
        "fastcdc-16k" => (
            ChunkMode::Cdc {
                min: 4 * 1024,
                avg: 16 * 1024,
                max: 64 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            },
            "fastcdc-16k",
        ),
        "fastcdc-32k" => (
            ChunkMode::Cdc {
                min: 8 * 1024,
                avg: 32 * 1024,
                max: 128 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            },
            "fastcdc-32k",
        ),
        "fastcdc-128k" => (
            ChunkMode::Cdc {
                min: 32 * 1024,
                avg: 128 * 1024,
                max: 512 * 1024,
                norm: cavs_chunker::NORM_DEFAULT,
            },
            "fastcdc-128k",
        ),
        "fastcdc-256k" => (
            ChunkMode::Cdc {
                min: 64 * 1024,
                avg: 256 * 1024,
                max: 1024 * 1024,
                norm: cavs_chunker::NORM_DEFAULT,
            },
            "fastcdc-256k",
        ),
        "fastcdc-64k-n3" => (
            ChunkMode::Cdc {
                min: 16 * 1024,
                avg: 64 * 1024,
                max: 256 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            },
            "fastcdc-64k-n3",
        ),
        "fastcdc-128k-n3" => (
            ChunkMode::Cdc {
                min: 32 * 1024,
                avg: 128 * 1024,
                max: 512 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            },
            "fastcdc-128k-n3",
        ),
        "fixed-256k" => (ChunkMode::Fixed { size: 256 * 1024 }, "fixed-256k"),
        "fixed-512k" => (ChunkMode::Fixed { size: 512 * 1024 }, "fixed-512k"),
        "fixed-1m" => (ChunkMode::Fixed { size: 1024 * 1024 }, "fixed-1m"),
        other => {
            return Err(SdkError::InvalidRequest(format!(
                "unknown profile '{other}'"
            )))
        }
    })
}

fn parse_compression(s: &str) -> Result<(bool, i32)> {
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
    Err(SdkError::InvalidRequest(format!(
        "unknown compression '{s}' (expected zstd-<1..22> or none)"
    )))
}

fn load_sign_key(path: &std::path::Path) -> Result<[u8; 32]> {
    let hex = std::fs::read_to_string(path)?;
    let hex = hex.trim();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidRequest(format!(
            "{} is not a 64-hex-char Ed25519 secret key",
            path.display()
        )));
    }
    let mut secret = [0u8; 32];
    for (i, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    Ok(secret)
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let started = std::time::Instant::now();
    let req: PackDirectoryRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    if !req.input_dir.is_dir() {
        return Err(SdkError::PathNotFound(req.input_dir.clone()));
    }
    let (mode, label) = parse_profile(&req.profile)?;
    let (compress, zstd_level) = parse_compression(&req.compression)?;
    let sign_key = req
        .sign_key_path
        .as_deref()
        .map(load_sign_key)
        .transpose()?;

    ctx.phase("scanning");
    let rules = IgnoreRules::load(&req.input_dir, &req.ignore)?;
    let entries = walk_sorted(&req.input_dir)?;
    let total_bytes_all: u64 = entries
        .iter()
        .filter_map(|rel| std::fs::symlink_metadata(req.input_dir.join(rel)).ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum();

    let uuid = *uuid::Uuid::new_v4().as_bytes();
    let mut w = Writer::create(&req.output_cavs, uuid, 1000, compress)?;
    w.set_zstd_level(zstd_level);
    if let Some(secret) = &sign_key {
        w.sign_with(secret);
    }
    w.set_meta(
        "packer",
        concat!("cavs-sdk-core ", env!("CARGO_PKG_VERSION")),
    );
    w.set_meta("payload", "directory");

    ctx.phase("chunking");
    let mut track_id = 0u32;
    let mut segment_id = 0u64;
    let mut files = 0u64;
    let mut ignored = 0u64;
    let mut done_bytes = 0u64;
    for rel in entries {
        ctx.check_cancelled()?;
        let full = req.input_dir.join(&rel);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !cavs_plan::path_is_safe(&rel_str) {
            return Err(SdkError::PathTraversal(rel_str));
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

        let data = std::fs::read(&full)?;
        files += 1;
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

        let ranges = cavs_chunker::split(&data, mode);
        let chunks = w.add_chunks_parallel(&data, &ranges)?;
        track_id += 1;
        w.add_track(TrackRecord {
            track_id,
            kind: TrackKind::Data,
            flags: 0,
            codec: "raw".to_string(),
            name: rel_str.clone(),
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
        })
        .map_err(|e| SdkError::Internal(e.to_string()))?;
        segment_id += 1;

        done_bytes += data.len() as u64;
        ctx.bytes("chunking", done_bytes, total_bytes_all, Some(rel_str));
    }

    if files == 0 {
        return Err(SdkError::InvalidRequest(format!(
            "{} contains no files",
            req.input_dir.display()
        )));
    }
    ctx.phase("finishing");
    let stats = w.finish().map_err(|e| SdkError::Internal(e.to_string()))?;

    Ok(json!({
        "outputCavs": req.output_cavs,
        "totalSizeBytes": stats.file_size,
        "chunkCount": stats.unique_chunks,
        "logicalChunks": stats.logical_chunks,
        "logicalRawBytes": stats.logical_raw,
        "storedBytes": stats.stored,
        "merkleRoot": cavs_hash::to_hex(&stats.merkle_root),
        "filesPacked": files,
        "entriesIgnored": ignored,
        "signed": sign_key.is_some(),
        "profile": label,
        "elapsedMs": started.elapsed().as_millis() as u64,
    }))
}
