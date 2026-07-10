//! Full build-transition analysis: the SteamPipe-style model, the
//! content-defined contrast model, per-file heatmaps/entropy, and the
//! detector/recommendation pass. This is the engine behind
//! `cavs analyze steampipe` and `cavs analyze-packs`.

use crate::detect::{detect_build, detect_file, FileSignals, Finding, Severity, Thresholds};
use crate::entropy;
use crate::steampipe::{self, ModelConfig};
use crate::walk::{mmap, walk};
use crate::windows::{heatmap, Heatmap};
use crate::Engine;
use anyhow::Result;
use cavs_chunker::ChunkMode;
use cavs_hash::hash_chunk;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

const CDC: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
    norm: cavs_chunker::NORM_DEFAULT,
};
/// Manifest overhead per CDC chunk (hash + offset + len + flags), analytic.
const CDC_MANIFEST_PER_CHUNK: u64 = 48;

/// Everything the reports need about one changed file.
#[derive(Serialize, Clone)]
pub struct FileAnalysis {
    pub path: String,
    /// new | modified
    pub status: String,
    pub is_pack: bool,
    pub old_size: u64,
    pub new_size: u64,
    // SteamPipe-style fixed 1 MiB model (same-path reuse).
    pub steam_new_chunks: u64,
    pub steam_total_chunks: u64,
    pub steam_reuse_ratio: f64,
    pub steam_download: u64,
    // CAVS FastCDC 64 KiB model (global reuse).
    pub cdc_new_chunks: u64,
    pub cdc_reuse_ratio: f64,
    pub cdc_download: u64,
    /// Sampled Shannon entropy of the new bytes, bits/byte.
    pub entropy_bits: f64,
    /// Positional heatmaps at 64 KiB / 1 MiB / 8 MiB windows.
    pub heat_64k: Heatmap,
    pub heat_1m: Heatmap,
    pub heat_8m: Heatmap,
}

/// The full analysis of one old→new transition.
#[derive(Serialize, Clone)]
pub struct Analysis {
    pub old_build: String,
    pub new_build: String,
    pub engine: String,
    pub old_size_bytes: u64,
    pub new_size_bytes: u64,
    pub files_unchanged: usize,
    pub files_modified: usize,
    pub files_added: usize,
    pub files_deleted: usize,
    pub deleted_paths: Vec<String>,
    pub estimated_steampipe_download: u64,
    pub estimated_cavs_download: u64,
    pub steam_reuse_ratio: f64,
    pub cdc_reuse_ratio: f64,
    /// Changed files ranked by SteamPipe-style download cost.
    pub files: Vec<FileAnalysis>,
    /// Detector output, most severe first.
    pub findings: Vec<Finding>,
    pub note: String,
}

impl Analysis {
    pub fn worst_severity(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }
}

/// Analyze `new_root` against `old_root`. `keep` filters relative paths.
pub fn analyze(
    old_root: &Path,
    new_root: &Path,
    engine: Engine,
    thresholds: &Thresholds,
    keep: &dyn Fn(&str) -> bool,
) -> Result<Analysis> {
    // The fixed-chunk model does the walking, indexing and per-file
    // download math once; we enrich its changed files with CDC, entropy
    // and heatmaps.
    let est = steampipe::estimate(old_root, new_root, &ModelConfig::default(), keep)?;

    // Global CDC index of the old build for the contrast model.
    let mut old_cdc: HashSet<[u8; 32]> = HashSet::new();
    for (rel, abs) in walk(old_root)?.into_iter().filter(|(r, _)| keep(r)) {
        let _ = rel;
        if let Some(map) = mmap(&abs)? {
            for range in cavs_chunker::split(&map, CDC) {
                old_cdc.insert(hash_chunk(&map[range]));
            }
        }
    }

    let mut files: Vec<FileAnalysis> = Vec::new();
    let mut cdc_total_download = 0u64;
    for fe in &est.files {
        let new_abs = if new_root.is_file() {
            new_root.to_path_buf()
        } else {
            new_root.join(&fe.path)
        };
        let old_abs = if old_root.is_file() {
            old_root.to_path_buf()
        } else {
            old_root.join(&fe.path)
        };
        let new_map = mmap(&new_abs)?;
        let new_bytes: &[u8] = new_map.as_deref().unwrap_or(&[]);
        let old_map = if old_abs.is_file() {
            mmap(&old_abs)?
        } else {
            None
        };
        let old_bytes: &[u8] = old_map.as_deref().unwrap_or(&[]);

        let mut cdc_new = 0u64;
        let mut cdc_total = 0u64;
        let mut cdc_download = 0u64;
        for range in cavs_chunker::split(new_bytes, CDC) {
            cdc_total += 1;
            let chunk = &new_bytes[range];
            if !old_cdc.contains(&hash_chunk(chunk)) {
                cdc_new += 1;
                cdc_download += zstd::bulk::compress(chunk, 3)
                    .map(|c| c.len() as u64)
                    .unwrap_or(chunk.len() as u64)
                    + CDC_MANIFEST_PER_CHUNK;
            }
        }
        let cdc_reuse = if cdc_total == 0 {
            1.0
        } else {
            1.0 - cdc_new as f64 / cdc_total as f64
        };
        cdc_total_download += cdc_download;

        files.push(FileAnalysis {
            path: fe.path.clone(),
            status: fe.status.clone(),
            is_pack: fe.is_pack,
            old_size: fe.old_size,
            new_size: fe.new_size,
            steam_new_chunks: fe.new_chunks,
            steam_total_chunks: fe.total_chunks,
            steam_reuse_ratio: fe.reuse_ratio,
            steam_download: fe.download_compressed,
            cdc_new_chunks: cdc_new,
            cdc_reuse_ratio: cdc_reuse,
            cdc_download,
            entropy_bits: entropy::sampled(new_bytes, 32),
            heat_64k: heatmap(old_bytes, new_bytes, 64 * 1024),
            heat_1m: heatmap(old_bytes, new_bytes, 1024 * 1024),
            heat_8m: heatmap(old_bytes, new_bytes, 8 * 1024 * 1024),
        });
    }

    // Detectors.
    let signals: Vec<FileSignals> = files
        .iter()
        .map(|f| FileSignals {
            path: f.path.clone(),
            status: f.status.clone(),
            is_pack: f.is_pack,
            old_size: f.old_size,
            new_size: f.new_size,
            fixed_reuse: f.steam_reuse_ratio,
            cdc_reuse: f.cdc_reuse_ratio,
            steam_download: f.steam_download,
            cdc_download: f.cdc_download,
            entropy: f.entropy_bits,
            heat_64k: f.heat_64k.clone(),
            heat_1m: f.heat_1m.clone(),
        })
        .collect();
    let mut findings: Vec<Finding> = Vec::new();
    for s in &signals {
        findings.extend(detect_file(s, thresholds));
    }
    findings.extend(detect_build(&signals, thresholds));
    if let Some(engine_hint) = engine_finding(&files, engine) {
        findings.push(engine_hint);
    }
    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));

    let reuse = |download: u64| {
        if est.new_size_bytes == 0 {
            1.0
        } else {
            (1.0 - download as f64 / est.new_size_bytes as f64).max(0.0)
        }
    };
    Ok(Analysis {
        old_build: est.old_build.clone(),
        new_build: est.new_build.clone(),
        engine: engine.name().into(),
        old_size_bytes: est.old_size_bytes,
        new_size_bytes: est.new_size_bytes,
        files_unchanged: est.files_unchanged,
        files_modified: est.files_modified,
        files_added: est.files_added,
        files_deleted: est.files_deleted,
        deleted_paths: est.deleted_paths.clone(),
        estimated_steampipe_download: est.estimated_download_compressed,
        estimated_cavs_download: cdc_total_download,
        steam_reuse_ratio: reuse(est.estimated_download_raw),
        cdc_reuse_ratio: reuse(cdc_total_download),
        files,
        findings,
        note: crate::ESTIMATE_NOTE.into(),
    })
}

/// Engine-specific advice when a problematic file belongs to a known
/// engine (or the caller forced one).
fn engine_finding(files: &[FileAnalysis], engine: Engine) -> Option<Finding> {
    let has_issue = |f: &FileAnalysis| f.steam_reuse_ratio < 0.6 && f.new_size > 16 << 20;
    let detected = files
        .iter()
        .filter(|f| has_issue(f))
        .find_map(|f| crate::engine_of(&f.path))
        .or(if engine != Engine::Auto {
            Some(engine)
        } else {
            None
        })?;
    let (title, fix) = match detected {
        Engine::Unreal => (
            "Unreal: check PAK/IoStore alignment",
            "Align patch padding to 1 MiB so PAK chunks match fixed-chunk boundaries, \
             split packs by map/feature, and inspect .ucas/.utoc containers for \
             offset churn.",
        ),
        Engine::Unity => (
            "Unity: stabilize AssetBundles/Addressables",
            "Group Addressables by update cadence, avoid moving assets between \
             bundles, keep naming/versioning stable, and avoid full bundle rebuilds \
             for small changes.",
        ),
        Engine::Godot => (
            "Godot: split the base PCK from update PCKs",
            "Ship a stable base PCK plus per-feature/level PCKs instead of rewriting \
             one monolithic PCK; load extra PCKs as resource packs at runtime.",
        ),
        _ => return None,
    };
    Some(Finding {
        severity: Severity::Info,
        kind: "engine_hint".into(),
        title: title.into(),
        file: None,
        estimated_wasted_bytes: 0,
        why: "Files with low fixed-chunk reuse belong to this engine's pack format.".into(),
        fix: fix.into(),
        expected_improvement: "Engine-native layout controls remove most fixed-chunk \
                               misalignment."
            .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep_all(_: &str) -> bool {
        true
    }

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    /// A shuffled pack: same 1 MiB assets, new order. CDC keeps reuse,
    /// the fixed model loses it, and the shuffling detector fires.
    #[test]
    fn shuffled_pack_produces_shuffling_finding() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let assets: Vec<Vec<u8>> = (0..24)
            .map(|i| pseudo_random(1 << 20, 100 + i as u32))
            .collect();
        let old: Vec<u8> = assets.concat();
        // Rotate by one asset plus a small offset so windows misalign.
        let mut new: Vec<u8> = vec![0x42; 4096];
        for a in assets.iter().skip(1).chain(assets.iter().take(1)) {
            new.extend_from_slice(a);
        }
        std::fs::write(a.join("world.pak"), &old).unwrap();
        std::fs::write(b.join("world.pak"), &new).unwrap();

        let analysis = analyze(&a, &b, Engine::Auto, &Thresholds::default(), &keep_all).unwrap();
        assert!(
            analysis
                .findings
                .iter()
                .any(|f| f.kind == "asset_shuffling"),
            "kinds: {:?}",
            analysis
                .findings
                .iter()
                .map(|f| f.kind.clone())
                .collect::<Vec<_>>()
        );
        // The CAVS estimate must be far below the fixed-chunk estimate.
        assert!(analysis.estimated_cavs_download * 4 < analysis.estimated_steampipe_download);
        assert!(analysis.note.contains("not Valve"));
    }

    #[test]
    fn localized_change_produces_no_critical_findings() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let old = pseudo_random(32 << 20, 5);
        let mut new = old.clone();
        new[10 << 20..(10 << 20) + 4096].copy_from_slice(&pseudo_random(4096, 6));
        std::fs::write(a.join("data.pak"), &old).unwrap();
        std::fs::write(b.join("data.pak"), &new).unwrap();

        let analysis = analyze(&a, &b, Engine::Auto, &Thresholds::default(), &keep_all).unwrap();
        assert!(analysis
            .findings
            .iter()
            .all(|f| f.severity < Severity::Critical));
        assert_eq!(analysis.files.len(), 1);
        assert!(analysis.files[0].steam_reuse_ratio > 0.9);
    }
}
