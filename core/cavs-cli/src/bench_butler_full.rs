//! `cavs bench butler-full` — benchmark the external butler binary's
//! *complete* patch pipeline on a real old/new pair (v0.8.0):
//!
//! ```text
//! butler diff                       default patch (rsync-style, brotli-1)
//! butler rediff --rediff-quality 9  optimized patch (bsdiff + brotli-9)
//! butler apply                      both patches, staged
//! butler verify                     both outputs
//! ```
//!
//! Where `bench butler-offline` measures only the default patch, this
//! harness also produces the optimized patch class with butler's own
//! `rediff` — the strongest patch the tool can generate locally. All raw
//! JSON lines, wall times and peak RSS are captured; both outputs are
//! verified byte-identical before any size is reported.

use crate::bench_butler::{copy_tree, tree_size, trees_identical};
use crate::report::human_bytes;
use crate::tool_metrics::{run_measured, version_line, MeasuredRun};
use anyhow::{bail, Context, Result};
use cavs_proto::errors::ErrorCode;
use std::path::Path;

#[derive(Default, serde::Serialize)]
pub struct ButlerFullReport {
    pub butler_version: Option<String>,
    pub mode: String,
    pub old_size_bytes: u64,
    pub new_size_bytes: u64,
    pub default_patch_bytes: u64,
    pub optimized_patch_bytes: u64,
    pub signature_bytes: u64,
    pub diff_ms: u64,
    pub rediff_ms: u64,
    pub apply_default_ms: u64,
    pub apply_optimized_ms: u64,
    pub verify_default_ms: u64,
    pub verify_optimized_ms: u64,
    pub diff_peak_rss_mib: Option<f64>,
    pub rediff_peak_rss_mib: Option<f64>,
    pub apply_default_peak_rss_mib: Option<f64>,
    pub apply_optimized_peak_rss_mib: Option<f64>,
    pub default_output_ok: bool,
    pub optimized_output_ok: bool,
    /// rediff can be unavailable in some builds; the default numbers stay
    /// valid and the reason is recorded here.
    pub rediff_skipped: Option<String>,
    pub notes: Vec<String>,
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
    if !report.default_output_ok {
        bail!(
            "{}",
            ErrorCode::ButlerVerifyFailed.msg("butler default apply output mismatch")
        );
    }
    Ok(())
}

pub fn run(old: &Path, new: &Path, butler_bin: &str, out: &Path) -> Result<ButlerFullReport> {
    if !crate::tool_metrics::available(butler_bin) {
        bail!(
            "{}",
            ErrorCode::ButlerNotFound
                .msg(format!("cannot execute {butler_bin:?}; pass --butler-bin"))
        );
    }
    std::fs::create_dir_all(out)?;
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

    let mut report = ButlerFullReport {
        butler_version: version_line(butler_bin, "-V"),
        mode: mode.into(),
        old_size_bytes: tree_size(&old_dir)?,
        new_size_bytes: tree_size(&new_dir)?,
        ..Default::default()
    };

    let patch = out.join("patch-default.pwr");
    let patch_opt = out.join("patch-optimized.pwr");
    let sig = out.join("patch-default.pwr.sig");

    // ---- Phase 1: default patch -------------------------------------------
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
    report.diff_peak_rss_mib = diff.peak_rss_mib;
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
    report.default_patch_bytes = std::fs::metadata(&patch)?.len();
    report.signature_bytes = std::fs::metadata(&sig)?.len();

    // ---- Phase 2: optimized patch (rediff, quality 9) ----------------------
    let rediff = save_jsonl(
        run_measured(
            butler_bin,
            &[
                "-j",
                "rediff",
                "--rediff-quality",
                "9",
                "--patch",
                &patch.display().to_string(),
                "--old",
                &old_dir.display().to_string(),
                "--new",
                &new_dir.display().to_string(),
                "--output",
                &patch_opt.display().to_string(),
            ],
            None,
        )?,
        &out.join("butler.rediff.jsonl"),
    )?;
    report.rediff_ms = rediff.wall_ms;
    report.rediff_peak_rss_mib = rediff.peak_rss_mib;
    if rediff.exit_ok && patch_opt.is_file() {
        report.optimized_patch_bytes = std::fs::metadata(&patch_opt)?.len();
    } else {
        report.rediff_skipped = Some(format!(
            "butler rediff exited {:?}: {}",
            rediff.exit_code,
            rediff.stderr.lines().last().unwrap_or_default()
        ));
    }

    // ---- Phase 3: apply + verify both patches ------------------------------
    let (apply_ms, apply_rss, verify_ms, ok) = apply_and_verify(
        butler_bin,
        &patch,
        &sig,
        &old_dir,
        &new_dir,
        &out.join("ApplyDefault"),
        staging.path().join("stage-default"),
        out,
        "default",
    )?;
    report.apply_default_ms = apply_ms;
    report.apply_default_peak_rss_mib = apply_rss;
    report.verify_default_ms = verify_ms;
    report.default_output_ok = ok;

    if report.rediff_skipped.is_none() {
        let (apply_ms, apply_rss, verify_ms, ok) = apply_and_verify(
            butler_bin,
            &patch_opt,
            &sig,
            &old_dir,
            &new_dir,
            &out.join("ApplyOptimized"),
            staging.path().join("stage-optimized"),
            out,
            "optimized",
        )?;
        report.apply_optimized_ms = apply_ms;
        report.apply_optimized_peak_rss_mib = apply_rss;
        report.verify_optimized_ms = verify_ms;
        report.optimized_output_ok = ok;
    }

    if mode == "artifact" {
        report
            .notes
            .push("single artifact benchmarked as a one-file folder".into());
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn apply_and_verify(
    butler_bin: &str,
    patch: &Path,
    sig: &Path,
    old_dir: &Path,
    new_dir: &Path,
    apply_out: &Path,
    butler_staging: std::path::PathBuf,
    out: &Path,
    label: &str,
) -> Result<(u64, Option<f64>, u64, bool)> {
    if apply_out.exists() {
        std::fs::remove_dir_all(apply_out)?;
    }
    copy_tree(old_dir, apply_out)?;
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
        &out.join(format!("butler.apply-{label}.jsonl")),
    )?;
    if !apply.exit_ok {
        bail!(
            "{}",
            ErrorCode::ButlerApplyFailed.msg(format!(
                "butler apply ({label}) exited {:?}: {}",
                apply.exit_code,
                apply.stderr.lines().last().unwrap_or_default()
            ))
        );
    }
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
        &out.join(format!("butler.verify-{label}.jsonl")),
    )?;
    let ok = verify.exit_ok && trees_identical(new_dir, apply_out)?;
    Ok((apply.wall_ms, apply.peak_rss_mib, verify.wall_ms, ok))
}

fn save_jsonl(run: MeasuredRun, path: &Path) -> Result<MeasuredRun> {
    std::fs::write(path, &run.stdout)?;
    Ok(run)
}

fn print_summary(r: &ButlerFullReport) {
    println!("bench butler-full ({} mode)", r.mode);
    if let Some(v) = &r.butler_version {
        println!("  butler version    : {v}");
    }
    println!(
        "  old / new         : {} / {}",
        human_bytes(r.old_size_bytes),
        human_bytes(r.new_size_bytes)
    );
    println!(
        "  default patch     : {} (diff {} ms, apply {} ms, {})",
        human_bytes(r.default_patch_bytes),
        r.diff_ms,
        r.apply_default_ms,
        if r.default_output_ok {
            "OK"
        } else {
            "MISMATCH"
        },
    );
    match &r.rediff_skipped {
        None => println!(
            "  optimized patch   : {} (rediff {} ms, apply {} ms, {})",
            human_bytes(r.optimized_patch_bytes),
            r.rediff_ms,
            r.apply_optimized_ms,
            if r.optimized_output_ok {
                "OK"
            } else {
                "MISMATCH"
            },
        ),
        Some(reason) => println!("  optimized patch   : skipped — {reason}"),
    }
    if let Some(rss) = r.rediff_peak_rss_mib {
        println!("  rediff peak RSS   : {rss:.0} MiB");
    }
    if let (Some(d), Some(o)) = (r.apply_default_peak_rss_mib, r.apply_optimized_peak_rss_mib) {
        println!("  apply peak RSS    : default {d:.0} MiB / optimized {o:.0} MiB");
    }
}

fn markdown(r: &ButlerFullReport) -> String {
    let mut md = String::new();
    md.push_str("# butler full-pipeline benchmark\n\n");
    if let Some(v) = &r.butler_version {
        md.push_str(&format!("butler: `{v}` — {} mode\n\n", r.mode));
    }
    md.push_str("| Metric | Default patch | Optimized patch (rediff q9) |\n|---|---:|---:|\n");
    md.push_str(&format!(
        "| patch bytes | {} | {} |\n",
        human_bytes(r.default_patch_bytes),
        if r.rediff_skipped.is_none() {
            human_bytes(r.optimized_patch_bytes)
        } else {
            "skipped".into()
        }
    ));
    md.push_str(&format!(
        "| generate | {} ms | {} ms |\n",
        r.diff_ms, r.rediff_ms
    ));
    md.push_str(&format!(
        "| generate peak RSS | {} | {} |\n",
        fmt_rss(r.diff_peak_rss_mib),
        fmt_rss(r.rediff_peak_rss_mib)
    ));
    md.push_str(&format!(
        "| apply | {} ms | {} ms |\n",
        r.apply_default_ms, r.apply_optimized_ms
    ));
    md.push_str(&format!(
        "| apply peak RSS | {} | {} |\n",
        fmt_rss(r.apply_default_peak_rss_mib),
        fmt_rss(r.apply_optimized_peak_rss_mib)
    ));
    md.push_str(&format!(
        "| output byte-identical | {} | {} |\n",
        if r.default_output_ok { "yes" } else { "**no**" },
        if r.rediff_skipped.is_none() {
            if r.optimized_output_ok {
                "yes"
            } else {
                "**no**"
            }
        } else {
            "—"
        }
    ));
    md.push_str(&format!(
        "\nsignature: {}\n",
        human_bytes(r.signature_bytes)
    ));
    if let Some(reason) = &r.rediff_skipped {
        md.push_str(&format!("\n> rediff skipped: {reason}\n"));
    }
    for n in &r.notes {
        md.push_str(&format!("\n> {n}\n"));
    }
    md
}

fn fmt_rss(v: Option<f64>) -> String {
    v.map(|m| format!("{m:.0} MiB"))
        .unwrap_or_else(|| "—".into())
}
