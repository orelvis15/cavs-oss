//! `cavs bench butler-offline` — benchmark the external `butler` binary's
//! offline diff/apply/verify pipeline on a real old/new pair, fairly.
//!
//! This harness does not reimplement butler: it drives a local `butler`
//! binary (`-j` JSON-lines mode), captures raw output, measures wall time
//! and peak RSS, and verifies the applied output byte-for-byte.
//!
//! Labeling rule: results are **butler default patch** numbers — the
//! rsync-style patch `butler diff` computes. The optimized patch class
//! (`butler rediff`) is measured separately by `cavs bench butler-full`.

use crate::report::human_bytes;
use crate::tool_metrics::{run_measured, version_line, MeasuredRun};
use anyhow::{bail, Context, Result};
use cavs_proto::errors::ErrorCode;
use std::path::Path;

#[derive(Default, serde::Serialize)]
pub struct ButlerReport {
    pub tool: String,
    pub butler_version: Option<String>,
    pub label: String,
    pub mode: String,
    pub old_size_bytes: u64,
    pub new_size_bytes: u64,
    pub patch_pwr_bytes: u64,
    pub signature_pwr_sig_bytes: u64,
    pub diff_ms: u64,
    pub apply_ms: u64,
    pub verify_ms: u64,
    pub total_ms: u64,
    pub peak_rss_mib: Option<f64>,
    pub output_matches: bool,
    pub exit_codes: ExitCodes,
    pub notes: Vec<String>,
}

#[derive(Default, serde::Serialize)]
pub struct ExitCodes {
    pub diff: Option<i32>,
    pub apply: Option<i32>,
    pub verify: Option<i32>,
}

pub fn bench(old: &Path, new: &Path, butler_bin: &str, out: &Path) -> Result<()> {
    let report = run(old, new, butler_bin, out)?;
    print_summary(&report);
    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::write(out.join("summary.md"), markdown(&report))?;
    println!("results : {}/summary.md + summary.json", out.display());
    if !report.output_matches {
        bail!(
            "{}",
            ErrorCode::ButlerVerifyFailed.msg("butler apply output does not match the new build")
        );
    }
    Ok(())
}

/// Run the pipeline and return the metrics (also used by `bench routes`).
pub fn run(old: &Path, new: &Path, butler_bin: &str, out: &Path) -> Result<ButlerReport> {
    if !crate::tool_metrics::available(butler_bin) {
        bail!(
            "{}",
            ErrorCode::ButlerNotFound.msg(format!(
                "cannot execute {butler_bin:?}; pass --butler-bin or install butler"
            ))
        );
    }
    std::fs::create_dir_all(out)?;

    // butler diff wants two builds; a single artifact is benchmarked as a
    // one-file folder (butler's own recommended shape is a folder anyway).
    let staging = tempfile::tempdir()?;
    let (old_dir, new_dir, mode) = if old.is_dir() && new.is_dir() {
        (old.to_path_buf(), new.to_path_buf(), "directory")
    } else if !old.is_dir() && !new.is_dir() {
        let od = staging.path().join("old");
        let nd = staging.path().join("new");
        std::fs::create_dir_all(&od)?;
        std::fs::create_dir_all(&nd)?;
        std::fs::copy(old, od.join(old.file_name().context("old name")?))?;
        std::fs::copy(new, nd.join(new.file_name().context("new name")?))?;
        (od, nd, "artifact")
    } else {
        bail!("--old and --new must both be files or both be directories");
    };

    let mut report = ButlerReport {
        tool: "butler".into(),
        butler_version: version_line(butler_bin, "-V"),
        label: "butler default patch (bench butler-full measures the rediff-optimized one)".into(),
        mode: mode.into(),
        old_size_bytes: tree_size(&old_dir)?,
        new_size_bytes: tree_size(&new_dir)?,
        ..Default::default()
    };
    if mode == "artifact" {
        report
            .notes
            .push("single artifact benchmarked as a one-file folder".into());
    }

    let patch = out.join("patch.pwr");
    let sig = out.join("patch.pwr.sig");

    // ---- diff ------------------------------------------------------------
    let diff = save_jsonl(
        run_measured(
            butler_bin,
            &[
                "-j",
                "diff",
                &old_dir.display().to_string(),
                &new_dir.display().to_string(),
                &patch.display().to_string(),
            ],
            None,
        )?,
        &out.join("butler.diff.jsonl"),
    )?;
    report.diff_ms = diff.wall_ms;
    report.exit_codes.diff = diff.exit_code;
    report.peak_rss_mib = max_opt(report.peak_rss_mib, diff.peak_rss_mib);
    if !diff.exit_ok {
        bail!(
            "{}",
            ErrorCode::ButlerDiffFailed.msg(format!(
                "butler diff exited {:?}: {}",
                diff.exit_code,
                diff.stderr.lines().last().unwrap_or_default()
            ))
        );
    }
    report.patch_pwr_bytes = std::fs::metadata(&patch)?.len();
    report.signature_pwr_sig_bytes = std::fs::metadata(&sig)?.len();

    // ---- apply -----------------------------------------------------------
    // `butler apply` patches a build in place (with its own staging dir);
    // apply onto a copy so the "old" input stays pristine.
    let apply_out = out.join("ApplyOut");
    if apply_out.exists() {
        std::fs::remove_dir_all(&apply_out)?;
    }
    copy_tree(&old_dir, &apply_out)?;
    let butler_staging = staging.path().join("butler-staging");
    let apply = save_jsonl(
        run_measured(
            butler_bin,
            &[
                "-j",
                "apply",
                "--staging-dir",
                &butler_staging.display().to_string(),
                &patch.display().to_string(),
                &apply_out.display().to_string(),
            ],
            None,
        )?,
        &out.join("butler.apply.jsonl"),
    )?;
    report.apply_ms = apply.wall_ms;
    report.exit_codes.apply = apply.exit_code;
    report.peak_rss_mib = max_opt(report.peak_rss_mib, apply.peak_rss_mib);
    if !apply.exit_ok {
        bail!(
            "{}",
            ErrorCode::ButlerApplyFailed.msg(format!(
                "butler apply exited {:?}: {}",
                apply.exit_code,
                apply.stderr.lines().last().unwrap_or_default()
            ))
        );
    }

    // ---- verify (butler's own check + an independent byte comparison) ----
    let verify = save_jsonl(
        run_measured(
            butler_bin,
            &[
                "-j",
                "verify",
                &sig.display().to_string(),
                &apply_out.display().to_string(),
            ],
            None,
        )?,
        &out.join("butler.verify.jsonl"),
    )?;
    report.verify_ms = verify.wall_ms;
    report.exit_codes.verify = verify.exit_code;
    report.peak_rss_mib = max_opt(report.peak_rss_mib, verify.peak_rss_mib);

    report.output_matches = verify.exit_ok && trees_identical(&new_dir, &apply_out)?;
    report.total_ms = report.diff_ms + report.apply_ms + report.verify_ms;

    // Inspection outputs (butler file / ls), saved raw for the record.
    for (name, args) in [
        ("butler.file.patch.jsonl", vec!["-j", "file"]),
        ("butler.ls.patch.jsonl", vec!["-j", "ls"]),
    ] {
        let mut full = args.clone();
        let p = patch.display().to_string();
        full.push(&p);
        if let Ok(run) = run_measured(butler_bin, &full, None) {
            let _ = std::fs::write(out.join(name), &run.stdout);
        }
    }
    Ok(report)
}

fn save_jsonl(run: MeasuredRun, path: &Path) -> Result<MeasuredRun> {
    std::fs::write(path, &run.stdout)?;
    Ok(run)
}

fn max_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (x, None) | (None, x) => x,
    }
}

pub fn tree_size(path: &Path) -> Result<u64> {
    if path.is_file() {
        return Ok(std::fs::metadata(path)?.len());
    }
    let mut total = 0u64;
    for rel in crate::compare::walk_sorted(path)? {
        let full = path.join(rel);
        let meta = std::fs::symlink_metadata(&full)?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for rel in crate::compare::walk_sorted(from)? {
        let src = from.join(&rel);
        let dst = to.join(&rel);
        let meta = std::fs::symlink_metadata(&src)?;
        if meta.is_dir() {
            std::fs::create_dir_all(&dst)?;
        } else if meta.is_file() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

pub fn trees_identical(a: &Path, b: &Path) -> Result<bool> {
    let files_a: Vec<_> = crate::compare::walk_sorted(a)?
        .into_iter()
        .filter(|r| a.join(r).is_file())
        .collect();
    for rel in &files_a {
        let fa = a.join(rel);
        let fb = b.join(rel);
        if !fb.is_file() {
            return Ok(false);
        }
        if std::fs::metadata(&fa)?.len() != std::fs::metadata(&fb)?.len() {
            return Ok(false);
        }
        if cavs_hash::hash_chunk(&std::fs::read(&fa)?)
            != cavs_hash::hash_chunk(&std::fs::read(&fb)?)
        {
            return Ok(false);
        }
    }
    let count_b = crate::compare::walk_sorted(b)?
        .into_iter()
        .filter(|r| b.join(r).is_file())
        .count();
    Ok(files_a.len() == count_b)
}

fn print_summary(r: &ButlerReport) {
    println!("bench butler-offline ({} mode)", r.mode);
    println!("  label            : {}", r.label);
    if let Some(v) = &r.butler_version {
        println!("  butler version   : {v}");
    }
    println!(
        "  old / new        : {} / {}",
        human_bytes(r.old_size_bytes),
        human_bytes(r.new_size_bytes)
    );
    println!("  patch.pwr        : {}", human_bytes(r.patch_pwr_bytes));
    println!(
        "  patch.pwr.sig    : {}",
        human_bytes(r.signature_pwr_sig_bytes)
    );
    println!(
        "  diff/apply/verify: {} / {} / {} ms",
        r.diff_ms, r.apply_ms, r.verify_ms
    );
    if let Some(rss) = r.peak_rss_mib {
        println!("  peak RSS         : {rss:.0} MiB");
    }
    println!(
        "  output matches   : {}",
        if r.output_matches { "yes" } else { "NO" }
    );
}

fn markdown(r: &ButlerReport) -> String {
    let mut md = String::new();
    md.push_str("# butler offline benchmark\n\n");
    md.push_str(&format!("> {}\n\n", r.label));
    if let Some(v) = &r.butler_version {
        md.push_str(&format!("butler: `{v}` — {} mode\n\n", r.mode));
    }
    md.push_str("| Metric | Value |\n|---|---:|\n");
    md.push_str(&format!(
        "| Old build | {} |\n",
        human_bytes(r.old_size_bytes)
    ));
    md.push_str(&format!(
        "| New build | {} |\n",
        human_bytes(r.new_size_bytes)
    ));
    md.push_str(&format!(
        "| patch.pwr | {} |\n",
        human_bytes(r.patch_pwr_bytes)
    ));
    md.push_str(&format!(
        "| patch.pwr.sig | {} |\n",
        human_bytes(r.signature_pwr_sig_bytes)
    ));
    md.push_str(&format!("| diff | {} ms |\n", r.diff_ms));
    md.push_str(&format!("| apply | {} ms |\n", r.apply_ms));
    md.push_str(&format!("| verify | {} ms |\n", r.verify_ms));
    if let Some(rss) = r.peak_rss_mib {
        md.push_str(&format!("| peak RSS | {rss:.0} MiB |\n"));
    }
    md.push_str(&format!(
        "| output byte-identical | {} |\n",
        if r.output_matches { "yes" } else { "**NO**" }
    ));
    for n in &r.notes {
        md.push_str(&format!("\n> {n}\n"));
    }
    md
}
