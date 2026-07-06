//! `cavs bench pairwise-proxy` — approximate the *optimized pairwise
//! patch* class (à la itch.io's backend bsdiff + high-quality Brotli)
//! with transparent local tools.
//!
//! Labeling rule: every number here is an **optimized pairwise proxy** —
//! bsdiff/xdelta3 plus a chosen recompression — not an official itch.io
//! backend result. Tools and versions are recorded; every apply is
//! verified byte-identical before its size is reported.

use crate::optimize_patch::compress;
use crate::report::human_bytes;
use crate::tool_metrics::{available, run_measured, version_line};
use anyhow::{bail, Result};
use cavs_proto::errors::ErrorCode;
use std::path::{Path, PathBuf};

#[derive(serde::Serialize)]
pub struct ProxyResult {
    pub method: String,
    pub patch_bytes_raw: u64,
    pub patch_bytes_compressed: u64,
    pub diff_ms: u64,
    pub apply_ms: u64,
    pub peak_rss_mib: Option<f64>,
    pub output_matches: bool,
}

#[derive(Default, serde::Serialize)]
pub struct ProxyReport {
    pub label: String,
    pub mode: String,
    pub old_size_bytes: u64,
    pub new_size_bytes: u64,
    pub tool_versions: Vec<(String, String)>,
    pub results: Vec<ProxyResult>,
    pub notes: Vec<String>,
}

pub fn bench(
    old: &Path,
    new: &Path,
    algos: &str,
    compressions: &str,
    out: Option<&Path>,
) -> Result<()> {
    let report = run(old, new, algos, compressions)?;
    print_report(&report);
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("pairwise-proxy.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        std::fs::write(dir.join("pairwise-proxy.md"), markdown(&report))?;
        println!("results : {}/pairwise-proxy.md + .json", dir.display());
    }
    Ok(())
}

pub fn run(old: &Path, new: &Path, algos: &str, compressions: &str) -> Result<ProxyReport> {
    let mut report = ProxyReport {
        label: "optimized pairwise proxy (NOT official itch.io backend results)".into(),
        mode: if old.is_dir() {
            "directory"
        } else {
            "artifact"
        }
        .into(),
        old_size_bytes: crate::bench_butler::tree_size(old)?,
        new_size_bytes: crate::bench_butler::tree_size(new)?,
        ..Default::default()
    };
    if old.is_dir() != new.is_dir() {
        bail!("--old and --new must both be files or both be directories");
    }

    let algos: Vec<&str> = algos
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let comps: Vec<&str> = compressions
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    for algo in &algos {
        let (diff_bin, apply_bin) = match *algo {
            "bsdiff" => ("bsdiff", "bspatch"),
            "xdelta3" => ("xdelta3", "xdelta3"),
            other => bail!("unknown algo {other} (use bsdiff, xdelta3)"),
        };
        if !available(diff_bin) || !available(apply_bin) {
            report
                .notes
                .push(ErrorCode::PairwiseToolMissing.msg(format!("{algo} not on PATH; skipped")));
            continue;
        }
        if *algo == "xdelta3" {
            if let Some(v) = version_line("xdelta3", "-V") {
                report.tool_versions.push(("xdelta3".into(), v));
            }
        } else {
            report
                .tool_versions
                .push(("bsdiff".into(), "system (no version flag)".into()));
        }

        // Pair up the work: one entry in artifact mode, per-file otherwise.
        let pairs = file_pairs(old, new)?;
        let tmp = tempfile::tempdir()?;
        let mut raw_total = 0u64;
        let mut diff_ms = 0u64;
        let mut apply_ms = 0u64;
        let mut peak: Option<f64> = None;
        let mut all_match = true;
        let mut raw_patches: Vec<Vec<u8>> = Vec::new();

        for (i, pair) in pairs.iter().enumerate() {
            match pair {
                Pair::Changed { old, new } => {
                    let patch = tmp.path().join(format!("p{i}.patch"));
                    let d = diff_one(algo, old, new, &patch)?;
                    diff_ms += d.wall_ms;
                    peak = max_opt(peak, d.peak_rss_mib);
                    if !d.exit_ok {
                        bail!("{algo} diff failed on {}", new.display());
                    }
                    raw_total += std::fs::metadata(&patch)?.len();

                    let rebuilt = tmp.path().join(format!("p{i}.out"));
                    let a = apply_one(algo, old, &patch, &rebuilt)?;
                    apply_ms += a.wall_ms;
                    peak = max_opt(peak, a.peak_rss_mib);
                    all_match &= a.exit_ok && std::fs::read(&rebuilt)? == std::fs::read(new)?;
                    raw_patches.push(std::fs::read(&patch)?);
                }
                Pair::NewFile { new } => {
                    // A file with no old counterpart ships whole.
                    raw_patches.push(std::fs::read(new)?);
                    raw_total += std::fs::metadata(new)?.len();
                }
            }
        }

        for comp in &comps {
            let mut compressed_total = 0u64;
            let mut failed = false;
            for raw in &raw_patches {
                match compress(raw, comp) {
                    Ok(c) => compressed_total += c.len().min(raw.len()) as u64,
                    Err(e) => {
                        report.notes.push(format!("{comp}: {e}; skipped"));
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                continue;
            }
            report.results.push(ProxyResult {
                method: format!("{algo}+{comp}"),
                patch_bytes_raw: raw_total,
                patch_bytes_compressed: compressed_total,
                diff_ms,
                apply_ms,
                peak_rss_mib: peak,
                output_matches: all_match,
            });
        }
    }
    report.notes.push(
        "pairwise patches serve exactly one old→new pair; storage and generation \
         cost grow with every published pair"
            .into(),
    );
    Ok(report)
}

enum Pair {
    Changed { old: PathBuf, new: PathBuf },
    NewFile { new: PathBuf },
}

fn file_pairs(old: &Path, new: &Path) -> Result<Vec<Pair>> {
    if !old.is_dir() {
        return Ok(vec![Pair::Changed {
            old: old.to_path_buf(),
            new: new.to_path_buf(),
        }]);
    }
    let mut out = Vec::new();
    for rel in crate::compare::walk_sorted(new)? {
        let new_full = new.join(&rel);
        if !new_full.is_file() {
            continue;
        }
        let old_full = old.join(&rel);
        if old_full.is_file() {
            // Identical files produce tiny patches; diff them anyway — that
            // is what a per-file pairwise pipeline does.
            out.push(Pair::Changed {
                old: old_full,
                new: new_full,
            });
        } else {
            out.push(Pair::NewFile { new: new_full });
        }
    }
    if out.is_empty() {
        bail!("{} contains no files", new.display());
    }
    Ok(out)
}

fn diff_one(
    algo: &str,
    old: &Path,
    new: &Path,
    patch: &Path,
) -> Result<crate::tool_metrics::MeasuredRun> {
    let o = old.display().to_string();
    let n = new.display().to_string();
    let p = patch.display().to_string();
    match algo {
        "bsdiff" => run_measured("bsdiff", &[&o, &n, &p], None),
        "xdelta3" => run_measured(
            "xdelta3",
            &["-e", "-9", "-f", "-S", "djw", "-s", &o, &n, &p],
            None,
        ),
        _ => unreachable!(),
    }
}

fn apply_one(
    algo: &str,
    old: &Path,
    patch: &Path,
    out: &Path,
) -> Result<crate::tool_metrics::MeasuredRun> {
    let o = old.display().to_string();
    let p = patch.display().to_string();
    let dst = out.display().to_string();
    match algo {
        "bsdiff" => run_measured("bspatch", &[&o, &dst, &p], None),
        "xdelta3" => run_measured("xdelta3", &["-d", "-f", "-s", &o, &p, &dst], None),
        _ => unreachable!(),
    }
}

fn max_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (x, None) | (None, x) => x,
    }
}

fn print_report(r: &ProxyReport) {
    println!("bench pairwise-proxy ({} mode)", r.mode);
    println!("  label : {}", r.label);
    println!(
        "  old / new : {} / {}",
        human_bytes(r.old_size_bytes),
        human_bytes(r.new_size_bytes)
    );
    for (tool, v) in &r.tool_versions {
        println!("  tool  : {tool} — {v}");
    }
    for res in &r.results {
        println!(
            "  {:<22} raw {} → {} | diff {} ms, apply {} ms{} | {}",
            res.method,
            human_bytes(res.patch_bytes_raw),
            human_bytes(res.patch_bytes_compressed),
            res.diff_ms,
            res.apply_ms,
            res.peak_rss_mib
                .map(|m| format!(", peak {m:.0} MiB"))
                .unwrap_or_default(),
            if res.output_matches { "OK" } else { "MISMATCH" }
        );
    }
    for n in &r.notes {
        println!("  note  : {n}");
    }
}

fn markdown(r: &ProxyReport) -> String {
    let mut md = String::new();
    md.push_str("# Optimized pairwise proxy benchmark\n\n");
    md.push_str(&format!("> {}\n\n", r.label));
    for (tool, v) in &r.tool_versions {
        md.push_str(&format!("- {tool}: `{v}`\n"));
    }
    md.push_str("\n| Method | Raw patch | Compressed | Diff | Apply | Peak RSS | Output |\n|---|---:|---:|---:|---:|---:|---|\n");
    for res in &r.results {
        md.push_str(&format!(
            "| {} | {} | {} | {} ms | {} ms | {} | {} |\n",
            res.method,
            human_bytes(res.patch_bytes_raw),
            human_bytes(res.patch_bytes_compressed),
            res.diff_ms,
            res.apply_ms,
            res.peak_rss_mib
                .map(|m| format!("{m:.0} MiB"))
                .unwrap_or_else(|| "—".into()),
            if res.output_matches {
                "OK"
            } else {
                "**MISMATCH**"
            }
        ));
    }
    md.push('\n');
    for n in &r.notes {
        md.push_str(&format!("> {n}\n"));
    }
    md
}
