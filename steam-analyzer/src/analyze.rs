//! Build analysis: index two builds, model SteamPipe (fixed 1 MiB) vs CAVS
//! (FastCDC 64 KiB) patch sizes, detect pack-file risk, and recommend fixes.
//!
//! All estimates are *predictive models*, not official Steam output — the
//! reports say so. SteamPipe splits files into ~1 MiB chunks, compresses and
//! diffs them; we reproduce that model and contrast it with content-defined
//! chunking to isolate how much update bloat comes from chunk misalignment.

use cavs_chunker::ChunkMode;
use cavs_hash::{hash_chunk, ChunkHash};
use memmap2::Mmap;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;

pub const STEAM_CHUNK: usize = 1024 * 1024; // SteamPipe ~1 MiB fixed chunks
const CDC: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
};
const ZSTD_LEVEL: i32 = 3;
/// Manifest overhead per CDC chunk (hash + offset + len + flags), analytic.
const CDC_MANIFEST_PER_CHUNK: u64 = 48;

/// Pack-file extensions across the major engines (case-insensitive).
const PACK_EXTS: &[&str] = &[
    "pak", "ucas", "utoc", "pck", "bundle", "assets", "ress", "archive", "big", "dat", "blob",
    "pack", "zip",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Auto,
    Unreal,
    Unity,
    Godot,
    Custom,
}

pub fn is_pack(path: &str) -> bool {
    match path.rsplit('.').next() {
        Some(ext) => PACK_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

fn engine_of(path: &str) -> Option<Engine> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "pak" | "ucas" | "utoc" => Some(Engine::Unreal),
        "bundle" | "assets" | "ress" => Some(Engine::Unity),
        "pck" => Some(Engine::Godot),
        _ => None,
    }
}

// --- indexing ---------------------------------------------------------------

/// Fixed 1 MiB chunk hashes of a byte slice, in order.
fn fixed_hashes(data: &[u8]) -> Vec<ChunkHash> {
    data.chunks(STEAM_CHUNK).map(hash_chunk).collect()
}

/// (hash, offset, len) for FastCDC chunks.
fn cdc_chunks(data: &[u8]) -> Vec<(ChunkHash, usize, usize)> {
    cavs_chunker::split(data, CDC)
        .into_iter()
        .map(|r| (hash_chunk(&data[r.clone()]), r.start, r.len()))
        .collect()
}

/// The v1 side we need to keep: per-file fixed-chunk sets (for same-path
/// reuse), plus global fixed and CDC sets (for moved-content detection).
pub struct OldIndex {
    pub per_file_fixed: HashMap<String, HashSet<ChunkHash>>,
    pub global_fixed: HashSet<ChunkHash>,
    pub global_cdc: HashSet<ChunkHash>,
    pub total_bytes: u64,
    pub files: usize,
}

/// Recursively list files under `root` as (relative-path, absolute-path).
pub fn walk(root: &Path) -> anyhow::Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    fn rec(
        base: &Path,
        dir: &Path,
        out: &mut Vec<(String, std::path::PathBuf)>,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                rec(base, &path, out)?;
            } else if ft.is_file() {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
        Ok(())
    }
    rec(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn mmap(path: &Path) -> anyhow::Result<Option<Mmap>> {
    let file = File::open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(None); // mmap of empty file is undefined
    }
    // SAFETY: analyzer reads a build tree it was pointed at; files are not
    // concurrently mutated during a run.
    Ok(Some(unsafe { Mmap::map(&file)? }))
}

pub fn index_old(root: &Path) -> anyhow::Result<OldIndex> {
    let mut idx = OldIndex {
        per_file_fixed: HashMap::new(),
        global_fixed: HashSet::new(),
        global_cdc: HashSet::new(),
        total_bytes: 0,
        files: 0,
    };
    for (rel, abs) in walk(root)? {
        idx.files += 1;
        let Some(map) = mmap(&abs)? else {
            idx.per_file_fixed.insert(rel, HashSet::new());
            continue;
        };
        idx.total_bytes += map.len() as u64;
        let fixed: HashSet<ChunkHash> = fixed_hashes(&map).into_iter().collect();
        idx.global_fixed.extend(&fixed);
        for (h, _, _) in cdc_chunks(&map) {
            idx.global_cdc.insert(h);
        }
        idx.per_file_fixed.insert(rel, fixed);
    }
    Ok(idx)
}

// --- per-file diff ----------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    None,
    Low,
    Medium,
    High,
}

impl RiskLevel {
    pub fn rank(self) -> u8 {
        match self {
            RiskLevel::None => 0,
            RiskLevel::Low => 1,
            RiskLevel::Medium => 2,
            RiskLevel::High => 3,
        }
    }
    pub fn label(self) -> &'static str {
        ["none", "low", "medium", "high"][self.rank() as usize]
    }
}

#[derive(Serialize, Clone)]
pub struct FileDiff {
    pub path: String,
    pub status: String, // new | changed | unchanged
    pub is_pack: bool,
    pub old_size: u64,
    pub new_size: u64,
    // SteamPipe fixed 1 MiB model (same-path).
    pub steam_new_chunks: u64,
    pub steam_total_chunks: u64,
    pub steam_reuse_ratio: f64,
    pub steam_payload_raw: u64,
    pub steam_payload_compressed: u64,
    pub changed_window_ratio: f64,
    // CAVS FastCDC 64 KiB model (global reuse).
    pub cdc_new_chunks: u64,
    pub cdc_reuse_ratio: f64,
    pub cdc_payload: u64,
    // Risk.
    pub risk: RiskLevel,
    pub reasons: Vec<String>,
}

/// Analyze one v2 file against the old index. Returns None for unchanged
/// files (identical hash) to keep reports focused.
fn diff_file(rel: &str, data: &[u8], v2_fixed: &[ChunkHash], old: &OldIndex) -> FileDiff {
    let new_size = data.len() as u64;
    let is_pack = is_pack(rel);
    let old_fixed = old.per_file_fixed.get(rel);
    let status = if old_fixed.is_none() {
        "new"
    } else {
        "changed"
    };

    // --- SteamPipe fixed model (same-path) ---
    let empty = HashSet::new();
    let same = old_fixed.unwrap_or(&empty);
    let mut steam_new = 0u64;
    let mut new_raw = 0u64;
    let mut new_compressed = 0u64;
    let mut off = 0usize;
    for h in v2_fixed {
        let len = STEAM_CHUNK.min(data.len() - off);
        if !same.contains(h) {
            steam_new += 1;
            new_raw += len as u64;
            new_compressed += zstd::bulk::compress(&data[off..off + len], ZSTD_LEVEL)
                .map(|c| c.len() as u64)
                .unwrap_or(len as u64);
        }
        off += len;
    }
    let steam_total = v2_fixed.len().max(1) as u64;
    let steam_reuse = 1.0 - steam_new as f64 / steam_total as f64;
    let changed_window_ratio = steam_new as f64 / steam_total as f64;

    // --- CAVS FastCDC model (global reuse) ---
    let mut cdc_new = 0u64;
    let mut cdc_payload = 0u64;
    let mut cdc_total = 0u64;
    for (h, o, len) in cdc_chunks(data) {
        cdc_total += 1;
        if !old.global_cdc.contains(&h) {
            cdc_new += 1;
            cdc_payload += zstd::bulk::compress(&data[o..o + len], ZSTD_LEVEL)
                .map(|c| c.len() as u64)
                .unwrap_or(len as u64)
                + CDC_MANIFEST_PER_CHUNK;
        }
    }
    let cdc_reuse = if cdc_total == 0 {
        1.0
    } else {
        1.0 - cdc_new as f64 / cdc_total as f64
    };

    // --- risk scoring ---
    let mut reasons = Vec::new();
    let mut risk = RiskLevel::None;
    let bump = |lvl: RiskLevel, why: &str, reasons: &mut Vec<String>, risk: &mut RiskLevel| {
        reasons.push(why.to_string());
        if lvl.rank() > risk.rank() {
            *risk = lvl;
        }
    };

    if status == "new" {
        bump(RiskLevel::Low, "new_file", &mut reasons, &mut risk);
    }
    if is_pack {
        if new_size > 8 << 30 {
            bump(RiskLevel::High, "large_pack_file", &mut reasons, &mut risk);
        } else if new_size > 2 << 30 {
            bump(
                RiskLevel::Medium,
                "large_pack_file",
                &mut reasons,
                &mut risk,
            );
        }
        // Scattered small changes across a big pack.
        if new_size > 256 << 20 && changed_window_ratio > 0.30 {
            bump(
                RiskLevel::High,
                "scattered_changes",
                &mut reasons,
                &mut risk,
            );
        }
    }
    // Misalignment: content is present but 1 MiB reuse is far below CDC reuse.
    if status == "changed" && cdc_reuse - steam_reuse > 0.25 && new_size > 16 << 20 {
        bump(
            RiskLevel::High,
            "cdc_reuse_much_higher_than_fixed_reuse",
            &mut reasons,
            &mut risk,
        );
    }
    // Whole-file rewrite of a large changed file.
    if status == "changed" && steam_reuse < 0.10 && new_size > 64 << 20 && cdc_reuse < 0.25 {
        bump(RiskLevel::Medium, "full_rewrite", &mut reasons, &mut risk);
    }

    FileDiff {
        path: rel.to_string(),
        status: status.to_string(),
        is_pack,
        old_size: 0, // filled by caller (needs old size)
        new_size,
        steam_new_chunks: steam_new,
        steam_total_chunks: steam_total,
        steam_reuse_ratio: steam_reuse,
        steam_payload_raw: new_raw,
        steam_payload_compressed: new_compressed,
        changed_window_ratio,
        cdc_new_chunks: cdc_new,
        cdc_reuse_ratio: cdc_reuse,
        cdc_payload,
        risk,
        reasons,
    }
}

#[derive(Serialize, Clone)]
pub struct Report {
    pub old_root: String,
    pub new_root: String,
    pub engine: String,
    pub old_size_bytes: u64,
    pub new_size_bytes: u64,
    pub changed_files: usize,
    pub new_files: usize,
    pub estimated_steam_update_bytes: u64,
    pub estimated_steam_update_raw_bytes: u64,
    pub estimated_cdc_update_bytes: u64,
    pub steam_reuse_ratio: f64,
    pub cdc_reuse_ratio: f64,
    pub risk: RiskLevel,
    pub top_offenders: Vec<FileDiff>,
    pub recommendations: Vec<Recommendation>,
    pub note: String,
}

#[derive(Serialize, Clone)]
pub struct Recommendation {
    pub severity: String,
    pub title: String,
    pub detail: String,
}

/// Compare `new_root` against `old_root` and produce the full report.
pub fn compare(old_root: &Path, new_root: &Path, engine: Engine) -> anyhow::Result<Report> {
    let old = index_old(old_root)?;
    // old sizes for reporting deltas.
    let mut old_sizes: HashMap<String, u64> = HashMap::new();
    for (rel, abs) in walk(old_root)? {
        old_sizes.insert(rel, std::fs::metadata(&abs)?.len());
    }

    let mut diffs: Vec<FileDiff> = Vec::new();
    let mut new_total = 0u64;
    let mut new_relpaths = HashSet::new();
    let mut steam_payload = 0u64;
    let mut steam_payload_raw = 0u64;
    let mut cdc_payload = 0u64;
    let mut changed_files = 0usize;
    let mut new_files = 0usize;

    for (rel, abs) in walk(new_root)? {
        new_relpaths.insert(rel.clone());
        let Some(map) = mmap(&abs)? else {
            new_total += 0;
            continue;
        };
        new_total += map.len() as u64;

        // Skip byte-identical files (same fixed-chunk set and same size).
        let old_fixed = old.per_file_fixed.get(&rel);
        let v2_first = fixed_hashes(&map);
        if let Some(of) = old_fixed {
            let same_size = old_sizes.get(&rel) == Some(&(map.len() as u64));
            if same_size && v2_first.iter().all(|h| of.contains(h)) && of.len() == v2_first.len() {
                continue; // unchanged
            }
        }

        let mut d = diff_file(&rel, &map, &v2_first, &old);
        d.old_size = old_sizes.get(&rel).copied().unwrap_or(0);
        if d.status == "new" {
            new_files += 1;
        } else {
            changed_files += 1;
        }
        steam_payload += d.steam_payload_compressed;
        steam_payload_raw += d.steam_payload_raw;
        cdc_payload += d.cdc_payload;
        diffs.push(d);
    }

    // Overall reuse ratios (weighted by changed data).
    let overall_risk =
        diffs.iter().map(|d| d.risk).fold(
            RiskLevel::None,
            |a, b| {
                if b.rank() > a.rank() {
                    b
                } else {
                    a
                }
            },
        );
    // Escalate risk if the whole update is a large fraction of the build.
    let risk = if new_total > 0 && steam_payload as f64 / new_total as f64 > 0.25 {
        RiskLevel::High
    } else {
        overall_risk
    };

    diffs.sort_by_key(|d| std::cmp::Reverse(d.steam_payload_compressed));
    let top_offenders: Vec<FileDiff> = diffs.iter().take(20).cloned().collect();

    let recommendations = recommend(&diffs, engine);
    let steam_reuse = ratio(steam_payload_raw, new_total);
    let cdc_reuse = ratio(cdc_payload, new_total);

    Ok(Report {
        old_root: old_root.display().to_string(),
        new_root: new_root.display().to_string(),
        engine: engine_name(engine),
        old_size_bytes: old.total_bytes,
        new_size_bytes: new_total,
        changed_files,
        new_files,
        estimated_steam_update_bytes: steam_payload,
        estimated_steam_update_raw_bytes: steam_payload_raw,
        estimated_cdc_update_bytes: cdc_payload,
        steam_reuse_ratio: 1.0 - steam_reuse,
        cdc_reuse_ratio: 1.0 - cdc_reuse,
        risk,
        top_offenders,
        recommendations,
        note: "SteamPipe estimate, not official Steam result. Fixed 1 MiB \
               model approximates SteamPipe; FastCDC 64 KiB is the CAVS model \
               shown for contrast."
            .to_string(),
    })
}

fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

fn engine_name(e: Engine) -> String {
    match e {
        Engine::Auto => "auto",
        Engine::Unreal => "unreal",
        Engine::Unity => "unity",
        Engine::Godot => "godot",
        Engine::Custom => "custom",
    }
    .to_string()
}

fn recommend(diffs: &[FileDiff], engine: Engine) -> Vec<Recommendation> {
    let mut recs = Vec::new();
    let has = |reason: &str| diffs.iter().any(|d| d.reasons.iter().any(|r| r == reason));

    if has("scattered_changes") || has("cdc_reuse_much_higher_than_fixed_reuse") {
        recs.push(Recommendation {
            severity: "high".into(),
            title: "Localize pack changes / avoid asset reordering".into(),
            detail: "A large pack shows changes scattered across the file (or content \
                     present but misaligned). SteamPipe's 1 MiB diff can't reuse chunks \
                     whose byte offsets shifted. Split the pack by level/feature, keep \
                     asset order stable between builds, centralize the TOC, and check for \
                     absolute offsets that cascade on small edits."
                .into(),
        });
    }
    if has("large_pack_file") {
        recs.push(Recommendation {
            severity: "medium".into(),
            title: "Split oversized pack files".into(),
            detail: "Pack files over ~2 GiB force SteamPipe to reconstruct huge files on \
                     the client even for small changes. Split into 1–2 GiB packs aligned \
                     to depots/features."
                .into(),
        });
    }
    if has("full_rewrite") {
        recs.push(Recommendation {
            severity: "medium".into(),
            title: "Avoid rewriting whole packs for small changes".into(),
            detail: "A large file was almost fully rewritten (very low chunk reuse). Add \
                     new assets to a new pack instead of rewriting existing packs, and \
                     disable non-deterministic compression/build metadata."
                .into(),
        });
    }
    // Engine-specific guidance when the offenders belong to a known engine.
    let offender_engine = diffs
        .iter()
        .filter(|d| d.risk.rank() >= RiskLevel::Medium.rank())
        .find_map(|d| engine_of(&d.path))
        .or(if engine != Engine::Auto {
            Some(engine)
        } else {
            None
        });
    match offender_engine {
        Some(Engine::Unreal) => recs.push(Recommendation {
            severity: "info".into(),
            title: "Unreal: check PAK/IoStore alignment".into(),
            detail: "Review patch padding alignment (1 MiB aligns PAK chunks to \
                     SteamPipe's block size), split packs by map/feature, and inspect \
                     .ucas/.utoc IoStore containers for offset churn."
                .into(),
        }),
        Some(Engine::Unity) => recs.push(Recommendation {
            severity: "info".into(),
            title: "Unity: stabilize AssetBundles/Addressables".into(),
            detail: "Group Addressables by update cadence, avoid moving assets between \
                     bundles, keep naming/versioning stable, and avoid full bundle \
                     rebuilds for small changes."
                .into(),
        }),
        Some(Engine::Godot) => recs.push(Recommendation {
            severity: "info".into(),
            title: "Godot: split base PCK from patches".into(),
            detail: "Ship a stable base PCK and per-feature/level patch PCKs instead of \
                     rewriting the whole PCK for small updates. CAVS Delivery can serve \
                     these updates externally."
                .into(),
        }),
        _ => {}
    }
    if recs.is_empty() {
        recs.push(Recommendation {
            severity: "info".into(),
            title: "No major SteamPipe risks detected".into(),
            detail: "Changes appear localized and chunk-aligned. The estimated update \
                     should patch efficiently."
                .into(),
        });
    }
    recs
}
