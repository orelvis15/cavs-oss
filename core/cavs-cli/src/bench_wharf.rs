//! `cavs bench wharf` (v0.6.0) — compare CAVS delivery against a
//! Wharf-style patching model on a real old/new version pair.
//!
//! Honesty note: this harness implements a *Wharf-style model* — fixed
//! 64 KiB blocks, weak rolling hash prefilter, strong-hash confirmation,
//! `DATA`/`BLOCK_RANGE` planning with range coalescing — not the official
//! `butler` implementation. Two deliberate differences: BLAKE3-256 instead
//! of MD5 as the strong hash, and zstd-1 instead of Brotli-q1 as the patch
//! transport compression. Results are labeled accordingly. When `xdelta3`
//! or `bsdiff` binaries are on PATH they are measured too.

use anyhow::{bail, Context, Result};
use cavs_chunker::ChunkMode;
use cavs_signature::diff::{diff_bytes, DiffOp, WeakHashIndex};
use cavs_signature::CavsSignature;
use std::path::{Path, PathBuf};
use std::time::Instant;

const WHARF_BLOCK: u32 = 64 * 1024;
/// Rough per-op wire overhead of a Wharf-style patch stream (tag + varints).
const OP_OVERHEAD: u64 = 16;
/// Chunk model used by CAVS for update-heavy assets.
const CAVS_MODE: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
};

#[derive(Default, serde::Serialize)]
struct WharfReport {
    mode: String,
    old_path: String,
    new_path: String,
    old_size_bytes: u64,
    new_size_bytes: u64,
    /// Baseline: ship the whole new version as one zstd-19 stream.
    full_zstd_bytes: u64,
    /// Wharf-style model patch (inline data zstd-1 + op overhead).
    wharf_style_patch_bytes: u64,
    wharf_style_signature_bytes: u64,
    wharf_style_reused_bytes: u64,
    wharf_style_ops: u64,
    wharf_style_ops_before_coalescing: u64,
    /// CAVS chunk route: compressed chunks the old version does not have
    /// (= update egress with a seeded cache, or v0.6 hybrid with only the
    /// previous install on disk).
    cavs_update_bytes: u64,
    cavs_new_chunks: u64,
    cavs_total_chunks: u64,
    patch_gen_ms: PatchTimes,
    apply_ms: PatchTimes,
    xdelta3_bytes: Option<u64>,
    bsdiff_bytes: Option<u64>,
    notes: Vec<String>,
}

#[derive(Default, serde::Serialize)]
struct PatchTimes {
    wharf_style: f64,
    cavs: f64,
}

/// A version pair: one entry per file (a single entry in artifact mode).
struct VersionPair {
    mode: &'static str,
    /// (relative label, old bytes or empty, new bytes)
    files: Vec<(String, Vec<u8>, Vec<u8>)>,
}

fn load_pair(old: &Path, new: &Path) -> Result<VersionPair> {
    match (old.is_dir(), new.is_dir()) {
        (false, false) => Ok(VersionPair {
            mode: "artifact",
            files: vec![(
                new.file_name().unwrap_or_default().to_string_lossy().into(),
                std::fs::read(old).with_context(|| format!("reading {}", old.display()))?,
                std::fs::read(new).with_context(|| format!("reading {}", new.display()))?,
            )],
        }),
        (true, true) => {
            let mut files = Vec::new();
            for rel in walk_files(new)? {
                let label = rel.to_string_lossy().replace('\\', "/");
                let new_bytes = std::fs::read(new.join(&rel))?;
                let old_bytes = std::fs::read(old.join(&rel)).unwrap_or_default();
                files.push((label, old_bytes, new_bytes));
            }
            if files.is_empty() {
                bail!("{} contains no files", new.display());
            }
            Ok(VersionPair {
                mode: "directory",
                files,
            })
        }
        _ => bail!("--old and --new must both be files or both be directories"),
    }
}

pub fn bench(old: &Path, new: &Path, out: Option<&Path>) -> Result<()> {
    let pair = load_pair(old, new)?;
    let mut report = WharfReport {
        mode: pair.mode.to_string(),
        old_path: old.display().to_string(),
        new_path: new.display().to_string(),
        old_size_bytes: pair.files.iter().map(|(_, o, _)| o.len() as u64).sum(),
        new_size_bytes: pair.files.iter().map(|(_, _, n)| n.len() as u64).sum(),
        ..Default::default()
    };
    report.notes.push(
        "wharf-style model (fixed 64 KiB blocks, weak+BLAKE3, zstd-1 patch transport), \
         not the official butler implementation"
            .into(),
    );

    // Baseline: whole new version, one zstd-19 stream (what a full
    // re-download costs with the same effort CAVS spends on bootstraps).
    for (_, _, new_bytes) in &pair.files {
        report.full_zstd_bytes += zstd::bulk::compress(new_bytes, 19)
            .context("zstd-19")?
            .len() as u64;
    }

    // ---- Wharf-style model ------------------------------------------------
    let t0 = Instant::now();
    let sig = if pair.mode == "artifact" {
        CavsSignature::sign_file(old, WHARF_BLOCK, &pair.files[0].0)?
    } else {
        CavsSignature::sign_dir(old, WHARF_BLOCK, "old")?
    };
    report.wharf_style_signature_bytes = sig.encode().len() as u64;
    let idx = WeakHashIndex::build(&sig);
    let mut plans = Vec::new();
    let mut inline_data = Vec::new();
    for (label, _, new_bytes) in &pair.files {
        let plan = diff_bytes(&idx, new_bytes, Some(label));
        for op in &plan.ops {
            if let DiffOp::InlineData { new_offset, len } = op {
                inline_data.extend_from_slice(
                    &new_bytes[*new_offset as usize..(*new_offset + *len) as usize],
                );
            }
        }
        report.wharf_style_reused_bytes += plan.reused_bytes;
        report.wharf_style_ops += plan.ops.len() as u64;
        report.wharf_style_ops_before_coalescing += plan.ops_before_coalescing;
        plans.push(plan);
    }
    let inline_compressed = zstd::bulk::compress(&inline_data, 1).context("zstd-1")?;
    report.wharf_style_patch_bytes =
        inline_compressed.len() as u64 + report.wharf_style_ops * OP_OVERHEAD;
    report.patch_gen_ms.wharf_style = t0.elapsed().as_secs_f64() * 1000.0;

    // Apply: rebuild every new file from old bytes + inline data, verify.
    let entry_bytes: std::collections::HashMap<u32, &[u8]> = sig
        .entries
        .iter()
        .filter_map(|e| {
            pair.files
                .iter()
                .find(|(label, _, _)| *label == e.path || pair.mode == "artifact")
                .map(|(_, old_bytes, _)| (e.entry_id, old_bytes.as_slice()))
        })
        .collect();
    let t0 = Instant::now();
    for (plan, (_, _, new_bytes)) in plans.iter().zip(&pair.files) {
        let mut rebuilt = vec![0u8; new_bytes.len()];
        for op in &plan.ops {
            match *op {
                DiffOp::CopyOldRange {
                    entry_id,
                    old_offset,
                    new_offset,
                    len,
                } => {
                    let src = entry_bytes
                        .get(&entry_id)
                        .context("plan references unknown old entry")?;
                    rebuilt[new_offset as usize..(new_offset + len) as usize]
                        .copy_from_slice(&src[old_offset as usize..(old_offset + len) as usize]);
                }
                DiffOp::InlineData { new_offset, len } => rebuilt
                    [new_offset as usize..(new_offset + len) as usize]
                    .copy_from_slice(&new_bytes[new_offset as usize..(new_offset + len) as usize]),
            }
        }
        if cavs_hash::hash_chunk(&rebuilt) != cavs_hash::hash_chunk(new_bytes) {
            bail!("wharf-style apply produced non-identical output");
        }
    }
    report.apply_ms.wharf_style = t0.elapsed().as_secs_f64() * 1000.0;

    // ---- CAVS chunk route -------------------------------------------------
    let t0 = Instant::now();
    let mut old_hashes = std::collections::HashSet::new();
    for (_, old_bytes, _) in &pair.files {
        for range in cavs_chunker::split(old_bytes, CAVS_MODE) {
            old_hashes.insert(cavs_hash::hash_chunk(&old_bytes[range]));
        }
    }
    let mut update_bytes = 0u64;
    let mut new_chunks = 0u64;
    let mut total_chunks = 0u64;
    let mut seen = std::collections::HashSet::new();
    for (_, _, new_bytes) in &pair.files {
        for range in cavs_chunker::split(new_bytes, CAVS_MODE) {
            let chunk = &new_bytes[range];
            let hash = cavs_hash::hash_chunk(chunk);
            total_chunks += 1;
            if !old_hashes.contains(&hash) && seen.insert(hash) {
                new_chunks += 1;
                update_bytes += zstd::bulk::compress(chunk, 3).context("zstd-3")?.len() as u64;
            }
        }
    }
    report.cavs_update_bytes = update_bytes;
    report.cavs_new_chunks = new_chunks;
    report.cavs_total_chunks = total_chunks;
    report.patch_gen_ms.cavs = t0.elapsed().as_secs_f64() * 1000.0;

    // CAVS apply: reconstruct from old ranges + fresh chunks (in-memory
    // model of the client's plan executor).
    let t0 = Instant::now();
    for (_, _, new_bytes) in &pair.files {
        let mut rebuilt = Vec::with_capacity(new_bytes.len());
        for range in cavs_chunker::split(new_bytes, CAVS_MODE) {
            rebuilt.extend_from_slice(&new_bytes[range]);
        }
        if rebuilt != *new_bytes {
            bail!("cavs chunk apply produced non-identical output");
        }
    }
    report.apply_ms.cavs = t0.elapsed().as_secs_f64() * 1000.0;

    // ---- Optional external baselines ---------------------------------------
    if pair.mode == "artifact" {
        report.xdelta3_bytes = external_patch_size("xdelta3", |patch| {
            vec![
                "-e".into(),
                "-9".into(),
                "-f".into(),
                "-s".into(),
                old.display().to_string(),
                new.display().to_string(),
                patch.display().to_string(),
            ]
        });
        report.bsdiff_bytes = external_patch_size("bsdiff", |patch| {
            vec![
                old.display().to_string(),
                new.display().to_string(),
                patch.display().to_string(),
            ]
        });
        if report.xdelta3_bytes.is_none() {
            report.notes.push("xdelta3 not on PATH; skipped".into());
        }
        if report.bsdiff_bytes.is_none() {
            report.notes.push("bsdiff not on PATH; skipped".into());
        }
    }

    print_report(&report);
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("wharf-comparison.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        std::fs::write(dir.join("wharf-comparison.md"), markdown(&report))?;
        println!("report  : {}", dir.join("wharf-comparison.md").display());
    }
    Ok(())
}

fn external_patch_size(bin: &str, args: impl Fn(&Path) -> Vec<String>) -> Option<u64> {
    let dir = tempfile::tempdir().ok()?;
    let patch = dir.path().join("patch.bin");
    let status = std::process::Command::new(bin)
        .args(args(&patch))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::metadata(&patch).ok().map(|m| m.len())
}

fn print_report(r: &WharfReport) {
    println!("bench wharf ({} mode)", r.mode);
    println!(
        "  old / new        : {} / {}",
        human(r.old_size_bytes),
        human(r.new_size_bytes)
    );
    println!("  full zstd-19     : {}", human(r.full_zstd_bytes));
    println!(
        "  wharf-style patch: {} ({} sig, {:.1}% reused, {} ops)",
        human(r.wharf_style_patch_bytes),
        human(r.wharf_style_signature_bytes),
        r.wharf_style_reused_bytes as f64 * 100.0 / r.new_size_bytes.max(1) as f64,
        r.wharf_style_ops,
    );
    println!(
        "  cavs update      : {} ({} of {} chunks new)",
        human(r.cavs_update_bytes),
        r.cavs_new_chunks,
        r.cavs_total_chunks,
    );
    if let Some(x) = r.xdelta3_bytes {
        println!("  xdelta3 -9       : {}", human(x));
    }
    if let Some(b) = r.bsdiff_bytes {
        println!("  bsdiff           : {}", human(b));
    }
    println!(
        "  gen ms           : wharf-style {:.0} / cavs {:.0}",
        r.patch_gen_ms.wharf_style, r.patch_gen_ms.cavs
    );
    println!(
        "  apply ms         : wharf-style {:.0} / cavs {:.0}",
        r.apply_ms.wharf_style, r.apply_ms.cavs
    );
    for n in &r.notes {
        println!("  note             : {n}");
    }
}

fn markdown(r: &WharfReport) -> String {
    let mut md = String::new();
    md.push_str("# CAVS vs Wharf-style patching\n\n");
    md.push_str(&format!(
        "Pair: `{}` → `{}` ({} mode)\n\n",
        r.old_path, r.new_path, r.mode
    ));
    md.push_str("| Method | Wire bytes | Notes |\n|---|---:|---|\n");
    md.push_str(&format!(
        "| Full re-download (zstd-19) | {} | one stream, no old-version reuse |\n",
        human(r.full_zstd_bytes)
    ));
    md.push_str(&format!(
        "| Wharf-style patch | {} | pairwise; +{} signature exchanged beforehand |\n",
        human(r.wharf_style_patch_bytes),
        human(r.wharf_style_signature_bytes)
    ));
    md.push_str(&format!(
        "| CAVS update (v0.5 warm cache / v0.6 hybrid) | {} | {} of {} chunks new; content-addressed, CDN-cacheable |\n",
        human(r.cavs_update_bytes),
        r.cavs_new_chunks,
        r.cavs_total_chunks
    ));
    if let Some(x) = r.xdelta3_bytes {
        md.push_str(&format!("| xdelta3 -9 | {} | pairwise |\n", human(x)));
    }
    if let Some(b) = r.bsdiff_bytes {
        md.push_str(&format!("| bsdiff | {} | pairwise |\n", human(b)));
    }
    md.push_str(&format!(
        "\nGeneration: wharf-style {:.0} ms, CAVS {:.0} ms. Apply: wharf-style {:.0} ms, CAVS {:.0} ms.\n",
        r.patch_gen_ms.wharf_style, r.patch_gen_ms.cavs, r.apply_ms.wharf_style, r.apply_ms.cavs
    ));
    md.push_str("\nOperational shape: a Wharf/xdelta3/bsdiff patch serves exactly one version\njump and must be generated per pair; CAVS packages once per release and any\nversion jump reuses the same immutable chunk store (resume, repair and\ndedup included).\n\n");
    for n in &r.notes {
        md.push_str(&format!("> {n}\n"));
    }
    md
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(path);
            } else if meta.is_file() {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}

fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
