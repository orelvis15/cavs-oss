//! Patch engine adapters for the policy benchmark (v1.1.0).
//!
//! The policy comparison is independent of the diff engine. `cavsplan`
//! is built in and always available; `bsdiff`, `xdelta3` and
//! `butler-offline` are driven as external subprocesses when installed.
//! A missing tool skips that engine with a recorded reason — it never
//! fails the benchmark.

use super::model::EdgeMeasure;
use anyhow::{bail, Context, Result};
use cavs_chunker::ChunkMode;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const CAVS_MODE: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
};

pub const KNOWN_ENGINES: &[&str] = &["cavsplan", "bsdiff", "xdelta3", "butler-offline"];

/// Engine availability probe; unavailable engines carry the reason.
pub fn availability(engines: &[String]) -> Vec<(String, Result<(), String>)> {
    engines
        .iter()
        .map(|e| {
            let status = match e.as_str() {
                "cavsplan" => Ok(()),
                "bsdiff" => {
                    if crate::tool_metrics::available("bsdiff")
                        && crate::tool_metrics::available("bspatch")
                    {
                        Ok(())
                    } else {
                        Err("bsdiff/bspatch not on PATH".to_string())
                    }
                }
                "xdelta3" => {
                    if crate::tool_metrics::available("xdelta3") {
                        Ok(())
                    } else {
                        Err("xdelta3 not on PATH".to_string())
                    }
                }
                "butler-offline" => {
                    if crate::tool_metrics::available("butler") {
                        Ok(())
                    } else {
                        Err("butler not on PATH".to_string())
                    }
                }
                other => Err(format!(
                    "unknown engine {other:?} (known: {})",
                    KNOWN_ENGINES.join(", ")
                )),
            };
            (e.clone(), status)
        })
        .collect()
}

pub fn tool_versions(engines: &[String]) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    out.insert(
        "cavsplan".into(),
        format!("cavs {}", env!("CARGO_PKG_VERSION")),
    );
    for e in engines {
        let line = match e.as_str() {
            "bsdiff" => crate::tool_metrics::version_line("bsdiff", "--version")
                .unwrap_or_else(|| "bsdiff (no version banner)".into()),
            "xdelta3" => crate::tool_metrics::version_line("xdelta3", "-V")
                .unwrap_or_else(|| "xdelta3 (no version banner)".into()),
            "butler-offline" => crate::tool_metrics::version_line("butler", "-V")
                .unwrap_or_else(|| "butler (no version banner)".into()),
            _ => continue,
        };
        out.insert(e.clone(), line);
    }
    out
}

/// Measure one old→new edge with one engine: diff, apply, verify.
/// `keep_patch_in` keeps the patch artifact for the raw/ directory.
pub fn measure_edge(
    engine: &str,
    old: &Path,
    new: &Path,
    zstd_level: i32,
    keep_patch_in: Option<&Path>,
) -> Result<EdgeMeasure> {
    match engine {
        "cavsplan" => measure_cavsplan(old, new, keep_patch_in),
        "bsdiff" => measure_bsdiff(old, new, zstd_level, keep_patch_in),
        "xdelta3" => measure_xdelta3(old, new, zstd_level, keep_patch_in),
        "butler-offline" => measure_butler(old, new, zstd_level, keep_patch_in),
        other => bail!("unknown engine {other:?}"),
    }
}

fn max_rss(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn keep(patch: &Path, dir: Option<&Path>, name: &str) -> Result<()> {
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
        std::fs::copy(patch, dir.join(name))?;
    }
    Ok(())
}

/// zstd of the patch artifact, reported alongside the raw size; engines
/// with internal compression usually don't shrink further.
fn recompressed(patch: &Path, level: i32) -> Result<u64> {
    let bytes = std::fs::read(patch)?;
    let compressed = zstd::bulk::compress(&bytes, level)?.len() as u64;
    Ok(compressed.min(bytes.len() as u64))
}

// ---- cavsplan (internal, exact, always available) -------------------------

fn measure_cavsplan(old: &Path, new: &Path, keep_dir: Option<&Path>) -> Result<EdgeMeasure> {
    let label = old
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let t0 = std::time::Instant::now();
    let sig = if old.is_dir() {
        cavs_signature::CavsSignature::sign_dir(old, cavs_signature::DEFAULT_BLOCK_SIZE, &label)?
    } else {
        cavs_signature::CavsSignature::sign_file(old, cavs_signature::DEFAULT_BLOCK_SIZE, &label)?
    };
    let plan = cavs_plan::build(&sig, new, &cavs_plan::BuildOptions::default())?;
    let encoded = plan.encode(19);
    let diff_ms = t0.elapsed().as_millis() as u64;
    let raw = encoded.len() as u64;

    let work = tempfile::tempdir()?;
    let plan_path = work.path().join("edge.cavsplan");
    std::fs::write(&plan_path, &encoded)?;
    keep(&plan_path, keep_dir, "edge.cavsplan")?;

    let opts = cavs_plan::apply::ApplyOptions {
        delete_removed: true,
        check_old: false,
        plan_path: None,
    };
    let t1 = std::time::Instant::now();
    let decoded = cavs_plan::OfflinePlan::decode(&encoded)?;
    let out = work.path().join("applied");
    if new.is_dir() {
        cavs_plan::apply::apply_dir(&decoded, old, &out, &opts)?;
    } else {
        cavs_plan::apply::apply_artifact(&decoded, old, &out)?;
    }
    let apply_ms = t1.elapsed().as_millis() as u64;

    let t2 = std::time::Instant::now();
    let verified = tree_hash(&out)? == tree_hash(new)?;
    let verify_ms = t2.elapsed().as_millis() as u64;

    Ok(EdgeMeasure {
        engine: "cavsplan".into(),
        raw_patch_bytes: raw,
        compressed_patch_bytes: raw, // encode() is already zstd-19
        diff_ms,
        apply_ms,
        verify_ms,
        peak_rss_mib: None,
        verified,
    })
}

// ---- bsdiff / xdelta3 (single artifacts only) ------------------------------

fn require_files(engine: &str, old: &Path, new: &Path) -> Result<()> {
    if old.is_dir() || new.is_dir() {
        bail!("{engine} diffs single artifacts; directory builds need cavsplan or butler-offline");
    }
    Ok(())
}

fn measure_bsdiff(
    old: &Path,
    new: &Path,
    zstd_level: i32,
    keep_dir: Option<&Path>,
) -> Result<EdgeMeasure> {
    require_files("bsdiff", old, new)?;
    let work = tempfile::tempdir()?;
    let patch = work.path().join("edge.bsdiff");
    let (o, n, p) = (path_str(old)?, path_str(new)?, path_str(&patch)?);

    let diff = crate::tool_metrics::run_measured("bsdiff", &[&o, &n, &p], None)?;
    if !diff.exit_ok {
        bail!("bsdiff exited {:?}: {}", diff.exit_code, diff.stderr.trim());
    }
    keep(&patch, keep_dir, "edge.bsdiff")?;

    let out = work.path().join("applied");
    let apply = crate::tool_metrics::run_measured("bspatch", &[&o, &path_str(&out)?, &p], None)?;
    if !apply.exit_ok {
        bail!(
            "bspatch exited {:?}: {}",
            apply.exit_code,
            apply.stderr.trim()
        );
    }

    let t = std::time::Instant::now();
    let verified = tree_hash(&out)? == tree_hash(new)?;
    Ok(EdgeMeasure {
        engine: "bsdiff".into(),
        raw_patch_bytes: std::fs::metadata(&patch)?.len(),
        compressed_patch_bytes: recompressed(&patch, zstd_level)?,
        diff_ms: diff.wall_ms,
        apply_ms: apply.wall_ms,
        verify_ms: t.elapsed().as_millis() as u64,
        peak_rss_mib: max_rss(diff.peak_rss_mib, apply.peak_rss_mib),
        verified,
    })
}

fn measure_xdelta3(
    old: &Path,
    new: &Path,
    zstd_level: i32,
    keep_dir: Option<&Path>,
) -> Result<EdgeMeasure> {
    require_files("xdelta3", old, new)?;
    let work = tempfile::tempdir()?;
    let patch = work.path().join("edge.xdelta3");
    let (o, n, p) = (path_str(old)?, path_str(new)?, path_str(&patch)?);

    let diff =
        crate::tool_metrics::run_measured("xdelta3", &["-e", "-9", "-f", "-s", &o, &n, &p], None)?;
    if !diff.exit_ok {
        bail!(
            "xdelta3 -e exited {:?}: {}",
            diff.exit_code,
            diff.stderr.trim()
        );
    }
    keep(&patch, keep_dir, "edge.xdelta3")?;

    let out = work.path().join("applied");
    let apply = crate::tool_metrics::run_measured(
        "xdelta3",
        &["-d", "-f", "-s", &o, &p, &path_str(&out)?],
        None,
    )?;
    if !apply.exit_ok {
        bail!(
            "xdelta3 -d exited {:?}: {}",
            apply.exit_code,
            apply.stderr.trim()
        );
    }

    let t = std::time::Instant::now();
    let verified = tree_hash(&out)? == tree_hash(new)?;
    Ok(EdgeMeasure {
        engine: "xdelta3".into(),
        raw_patch_bytes: std::fs::metadata(&patch)?.len(),
        compressed_patch_bytes: recompressed(&patch, zstd_level)?,
        diff_ms: diff.wall_ms,
        apply_ms: apply.wall_ms,
        verify_ms: t.elapsed().as_millis() as u64,
        peak_rss_mib: max_rss(diff.peak_rss_mib, apply.peak_rss_mib),
        verified,
    })
}

// ---- butler (external; builds are folders) ---------------------------------

fn measure_butler(
    old: &Path,
    new: &Path,
    zstd_level: i32,
    keep_dir: Option<&Path>,
) -> Result<EdgeMeasure> {
    let work = tempfile::tempdir()?;
    // butler diffs folders; single artifacts are wrapped in one-file dirs
    // (butler's own recommended shape).
    let old_dir = as_dir(old, &work.path().join("old"))?;
    let new_dir = as_dir(new, &work.path().join("new"))?;
    let patch = work.path().join("edge.pwr");

    let diff = crate::tool_metrics::run_measured(
        "butler",
        &[
            "diff",
            "--json",
            &path_str(&old_dir)?,
            &path_str(&new_dir)?,
            &path_str(&patch)?,
        ],
        None,
    )?;
    if !diff.exit_ok {
        bail!(
            "butler diff exited {:?}: {}",
            diff.exit_code,
            diff.stderr.trim()
        );
    }
    keep(&patch, keep_dir, "edge.pwr")?;

    // butler apply patches in place: apply onto a copy of the old build.
    let target = work.path().join("applied");
    copy_tree(&old_dir, &target)?;
    let staging = work.path().join("staging");
    let apply = crate::tool_metrics::run_measured(
        "butler",
        &[
            "apply",
            "--json",
            "--staging-dir",
            &path_str(&staging)?,
            &path_str(&patch)?,
            &path_str(&target)?,
        ],
        None,
    )?;
    if !apply.exit_ok {
        bail!(
            "butler apply exited {:?}: {}",
            apply.exit_code,
            apply.stderr.trim()
        );
    }

    let t = std::time::Instant::now();
    let verified = tree_hash(&target)? == tree_hash(&new_dir)?;
    Ok(EdgeMeasure {
        engine: "butler-offline".into(),
        raw_patch_bytes: std::fs::metadata(&patch)?.len(),
        compressed_patch_bytes: recompressed(&patch, zstd_level)?,
        diff_ms: diff.wall_ms,
        apply_ms: apply.wall_ms,
        verify_ms: t.elapsed().as_millis() as u64,
        peak_rss_mib: max_rss(diff.peak_rss_mib, apply.peak_rss_mib),
        verified,
    })
}

fn as_dir(build: &Path, scratch: &Path) -> Result<PathBuf> {
    if build.is_dir() {
        return Ok(build.to_path_buf());
    }
    std::fs::create_dir_all(scratch)?;
    let name = build
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("build path has no file name"))?;
    std::fs::copy(build, scratch.join(name))?;
    Ok(scratch.to_path_buf())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for (rel, abs) in cavs_analyzer::walk::walk(from)? {
        let dst = to.join(&rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&abs, &dst)?;
    }
    Ok(())
}

fn path_str(p: &Path) -> Result<String> {
    Ok(p.to_string_lossy().to_string())
}

/// BLAKE3 over a file's content, or over the sorted (path, content-hash)
/// pairs of a directory — enough to compare two trees byte-for-byte.
pub fn tree_hash(path: &Path) -> Result<[u8; 32]> {
    let mut hasher = cavs_hash::Hasher::new();
    if path.is_dir() {
        for (rel, abs) in cavs_analyzer::walk::walk(path)? {
            hasher.update(rel.as_bytes());
            hasher.update(&cavs_hash::hash_chunk(&std::fs::read(&abs)?));
        }
    } else {
        hasher.update(&cavs_hash::hash_chunk(&std::fs::read(path)?));
    }
    Ok(hasher.finalize())
}

// ---- CAVS content-addressed measurement -------------------------------------

/// Per-version chunk inventory for the CAVS route: hash set plus
/// compressed (zstd-3, the pack default) size of every unique chunk.
pub struct CavsInventory {
    /// version index → set of chunk hashes.
    pub version_chunks: Vec<HashSet<[u8; 32]>>,
    /// chunk hash → compressed stored size.
    pub chunk_bytes: HashMap<[u8; 32], u64>,
    /// Time spent chunking+hashing+compressing all versions (the CAVS
    /// "build" cost — there is no per-pair diff step).
    pub build_ms: u64,
}

pub fn cavs_inventory(paths: &[PathBuf]) -> Result<CavsInventory> {
    let t0 = std::time::Instant::now();
    let mut version_chunks = Vec::with_capacity(paths.len());
    let mut chunk_bytes: HashMap<[u8; 32], u64> = HashMap::new();
    for path in paths {
        let mut set = HashSet::new();
        let files: Vec<PathBuf> = if path.is_dir() {
            cavs_analyzer::walk::walk(path)?
                .into_iter()
                .map(|(_, abs)| abs)
                .collect()
        } else {
            vec![path.clone()]
        };
        for file in files {
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            for range in cavs_chunker::split(&bytes, CAVS_MODE) {
                let chunk = &bytes[range];
                let hash = cavs_hash::hash_chunk(chunk);
                if let std::collections::hash_map::Entry::Vacant(slot) = chunk_bytes.entry(hash) {
                    slot.insert(zstd::bulk::compress(chunk, 3)?.len() as u64);
                }
                set.insert(hash);
            }
        }
        version_chunks.push(set);
    }
    Ok(CavsInventory {
        version_chunks,
        chunk_bytes,
        build_ms: t0.elapsed().as_millis() as u64,
    })
}

impl CavsInventory {
    /// Deduplicated store size across every version.
    pub fn store_bytes(&self) -> u64 {
        self.chunk_bytes.values().sum()
    }

    /// Bytes served for a from→to update when the client's local source
    /// is exactly version `from` (cold cache + previous install).
    pub fn update_bytes(&self, from: usize, to: usize) -> u64 {
        self.fresh_bytes(&self.version_chunks[from], to)
    }

    /// Bytes served when the cache accumulated every version ≤ `from`.
    pub fn warm_update_bytes(&self, from: usize, to: usize) -> u64 {
        let mut have = HashSet::new();
        for set in &self.version_chunks[..=from] {
            have.extend(set.iter().copied());
        }
        self.fresh_bytes(&have, to)
    }

    /// Full (re)install of version `to` from the store.
    pub fn install_bytes(&self, to: usize) -> u64 {
        self.version_chunks[to]
            .iter()
            .map(|h| self.chunk_bytes[h])
            .sum()
    }

    fn fresh_bytes(&self, have: &HashSet<[u8; 32]>, to: usize) -> u64 {
        self.version_chunks[to]
            .iter()
            .filter(|h| !have.contains(*h))
            .map(|h| self.chunk_bytes[h])
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_versions(dir: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let mut base = crate::patch_policy_bench::test_bytes(512 * 1024, 7);
        for v in 0..3 {
            // each version rewrites a different 64 KiB span
            let start = v * 128 * 1024;
            let patch = crate::patch_policy_bench::test_bytes(64 * 1024, 100 + v as u64);
            base[start..start + patch.len()].copy_from_slice(&patch);
            let p = dir.join(format!("v{v}.bin"));
            std::fs::write(&p, &base).unwrap();
            paths.push(p);
        }
        paths
    }

    #[test]
    fn cavsplan_engine_measures_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let paths = write_versions(dir.path());
        let m = measure_edge("cavsplan", &paths[0], &paths[1], 19, None).unwrap();
        assert!(m.verified);
        assert!(m.raw_patch_bytes > 0);
        assert!(m.compressed_patch_bytes <= 512 * 1024);
    }

    #[test]
    fn cavs_inventory_serves_updates_and_reinstalls() {
        let dir = tempfile::tempdir().unwrap();
        let paths = write_versions(dir.path());
        let inv = cavs_inventory(&paths).unwrap();
        let adjacent = inv.update_bytes(0, 1);
        let jump = inv.update_bytes(0, 2);
        let install = inv.install_bytes(2);
        assert!(adjacent > 0 && adjacent < install);
        assert!(jump >= adjacent && jump < install);
        // warm cache can only shrink the update
        assert!(inv.warm_update_bytes(1, 2) <= inv.update_bytes(1, 2));
        assert!(inv.store_bytes() >= install);
    }

    #[test]
    fn missing_tools_are_reported_not_fatal() {
        let engines = vec!["cavsplan".to_string(), "no-such-engine".to_string()];
        let avail = availability(&engines);
        assert!(avail[0].1.is_ok());
        assert!(avail[1].1.is_err());
    }
}
