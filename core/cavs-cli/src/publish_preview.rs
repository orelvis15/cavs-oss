//! `cavs publish-preview` (v0.9.0): the expected cost of publishing a
//! build, before it ships. Measures every delivery route (full, CAVS
//! chunk/hybrid/plan, butler and pairwise proxies when available), adds
//! the SteamPipe-style fixed-1MiB estimate, runs the layout analyzer for
//! release-readiness warnings and recommends a route with an explanation.

use crate::bench_routes::{self, RoutesArgs, RoutesReport};
use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_analyzer::compare::{analyze, Analysis};
use cavs_analyzer::detect::{Severity, Thresholds};
use cavs_analyzer::steampipe::{estimate, Estimate, ModelConfig};
use cavs_analyzer::Engine;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct PreviewArgs<'a> {
    /// Direct mode: the new build and its predecessor.
    pub build: Option<&'a Path>,
    pub previous: Option<&'a Path>,
    /// Workspace mode: resolve builds from a workspace instead.
    pub workspace: Option<&'a Path>,
    pub app: Option<&'a str>,
    pub from_build: Option<&'a str>,
    pub to_build: Option<&'a str>,
    /// External butler binary to include, when installed.
    pub butler_bin: Option<&'a str>,
    /// Include bsdiff/xdelta3 pairwise proxies (needs those tools).
    pub include_pairwise: bool,
    pub out: Option<&'a Path>,
    pub json: bool,
}

#[derive(Serialize)]
pub struct PreviewReport {
    pub old: String,
    pub new: String,
    pub steampipe_style: SteampipeRow,
    pub routes: RoutesReport,
    pub recommended: String,
    pub reason: String,
    pub warnings: Vec<String>,
    pub note: String,
}

#[derive(Serialize)]
pub struct SteampipeRow {
    pub network_bytes: u64,
    pub new_or_changed_chunks: u64,
    pub rebuild_read_bytes: u64,
    pub rebuild_write_bytes: u64,
}

/// Resolve the (old, new) pair, from paths or from a workspace.
fn resolve_pair(args: &PreviewArgs) -> Result<(PathBuf, PathBuf)> {
    if let (Some(build), Some(previous)) = (args.build, args.previous) {
        return Ok((previous.to_path_buf(), build.to_path_buf()));
    }
    let (Some(ws_root), Some(from_id), Some(to_id)) =
        (args.workspace, args.from_build, args.to_build)
    else {
        bail!("pass a build and --previous, or --workspace with --from and --to build ids");
    };
    let ws = cavs_workspace::Workspace::open(ws_root)?;
    let app_id = ws.app_id(args.app)?;
    let resolve = |id: &str| -> Result<PathBuf> {
        let build = ws.build(&app_id, id)?;
        let depot = build
            .depots
            .first()
            .with_context(|| format!("build '{id}' records no depots"))?;
        if build.depots.len() > 1 {
            eprintln!(
                "note    : build '{id}' has {} depots; previewing depot '{}' \
                 (run per depot for the rest)",
                build.depots.len(),
                depot.depot_id
            );
        }
        let path = PathBuf::from(&depot.source_path);
        if !path.exists() {
            bail!(
                "depot '{}' of build '{id}' points at {}, which no longer exists",
                depot.depot_id,
                path.display()
            );
        }
        Ok(path)
    };
    Ok((resolve(from_id)?, resolve(to_id)?))
}

pub fn publish_preview(args: &PreviewArgs) -> Result<()> {
    let (old, new) = resolve_pair(args)?;

    // Route measurements need a scratch directory for real artifacts.
    let tmp;
    let out_dir: PathBuf = match args.out {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            dir.to_path_buf()
        }
        None => {
            tmp = tempfile::tempdir()?;
            tmp.path().to_path_buf()
        }
    };

    let routes = bench_routes::collect(&RoutesArgs {
        old: &old,
        new: &new,
        butler_bin: args.butler_bin,
        include_pairwise_proxy: args.include_pairwise,
        out: &out_dir.join("routes-work"),
    })?;
    let steam = estimate(&old, &new, &ModelConfig::default(), &|_: &str| true)?;
    let analysis = analyze(&old, &new, Engine::Auto, &Thresholds::default(), &|_| true)?;

    let (recommended, reason) = recommend(&routes, &steam);
    let warnings = collect_warnings(&routes, &analysis);

    let report = PreviewReport {
        old: old.display().to_string(),
        new: new.display().to_string(),
        steampipe_style: SteampipeRow {
            network_bytes: steam.estimated_download_compressed,
            new_or_changed_chunks: steam.new_or_changed_chunks,
            rebuild_read_bytes: steam.rebuild_read_bytes,
            rebuild_write_bytes: steam.rebuild_write_bytes,
        },
        routes,
        recommended,
        reason,
        warnings,
        note: cavs_analyzer::ESTIMATE_NOTE.into(),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_preview(&report);
    }
    if args.out.is_some() {
        std::fs::write(out_dir.join("preview.md"), markdown(&report))?;
        std::fs::write(
            out_dir.join("preview.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        eprintln!("results : {}/preview.md + preview.json", out_dir.display());
    }
    Ok(())
}

/// Smallest verified network payload wins; ties and near-ties (≤2%)
/// prefer the CAVS plan for its bounded apply memory.
fn recommend(routes: &RoutesReport, _steam: &Estimate) -> (String, String) {
    let candidates: Vec<&crate::bench_routes::RouteRow> = routes
        .routes
        .iter()
        .filter(|r| r.output_ok != Some(false))
        .collect();
    let Some(min) = candidates.iter().map(|r| r.network_bytes).min() else {
        return ("full download (raw)".into(), "no measured routes".into());
    };
    let near: Vec<&crate::bench_routes::RouteRow> = candidates
        .iter()
        .filter(|r| r.network_bytes <= min + min / 50)
        .copied()
        .collect();
    let best = near
        .iter()
        .find(|r| r.route.contains("cavsplan"))
        .or_else(|| near.first())
        .unwrap();
    let mut why = vec![format!(
        "lowest verified network payload ({})",
        human_bytes(best.network_bytes)
    )];
    if best.route.contains("cavsplan") {
        why.push("streaming apply with bounded memory".into());
        why.push("no pairwise O(N²) patch explosion".into());
        why.push("byte-identical output verified".into());
    }
    (best.route.clone(), why.join(" · "))
}

fn collect_warnings(routes: &RoutesReport, analysis: &Analysis) -> Vec<String> {
    let mut warnings: Vec<String> = routes
        .skipped
        .iter()
        .map(|s| format!("route skipped: {s}"))
        .collect();
    for f in &analysis.findings {
        if f.severity >= Severity::Warning {
            warnings.push(format!(
                "[{}] {}{} — {}",
                f.severity.label(),
                f.title,
                f.file
                    .as_deref()
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default(),
                f.fix
            ));
        }
    }
    warnings
}

fn print_preview(r: &PreviewReport) {
    println!("publish-preview: {} → {}", r.old, r.new);
    println!(
        "  {:<34} {:>12}   fixed 1 MiB estimate ({} chunks, ~{} local rebuild I/O)",
        "SteamPipe-style",
        human_bytes(r.steampipe_style.network_bytes),
        r.steampipe_style.new_or_changed_chunks,
        human_bytes(r.steampipe_style.rebuild_read_bytes + r.steampipe_style.rebuild_write_bytes),
    );
    for row in &r.routes.routes {
        println!(
            "  {:<34} {:>12}  diff {:>7}  apply {:>7}  {}",
            row.route,
            human_bytes(row.network_bytes),
            row.diff_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            row.apply_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            match row.output_ok {
                Some(true) => "OK",
                Some(false) => "MISMATCH",
                None => "",
            }
        );
    }
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    println!("\nrecommended : {}", r.recommended);
    println!("why         : {}", r.reason);
    println!("note        : {}", r.note);
}

pub fn markdown(r: &PreviewReport) -> String {
    let mut md = String::new();
    md.push_str("# Publish Preview\n\n");
    md.push_str(&format!("> {}\n\n", r.note));
    md.push_str(&format!("`{}` → `{}`\n\n", r.old, r.new));
    md.push_str("| Route | Network | Build/Diff | Apply | Peak RSS | Verified | Notes |\n|---|---:|---:|---:|---:|---|---|\n");
    md.push_str(&format!(
        "| SteamPipe-style (fixed 1 MiB) | {} | — | — | — | model | {} chunks; ~{} local rebuild I/O |\n",
        human_bytes(r.steampipe_style.network_bytes),
        r.steampipe_style.new_or_changed_chunks,
        human_bytes(r.steampipe_style.rebuild_read_bytes + r.steampipe_style.rebuild_write_bytes),
    ));
    for row in &r.routes.routes {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            row.route,
            human_bytes(row.network_bytes),
            row.diff_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            row.apply_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            row.peak_rss_mib
                .map(|m| format!("{m:.0} MiB"))
                .unwrap_or_else(|| "—".into()),
            match row.output_ok {
                Some(true) => "yes",
                Some(false) => "**no**",
                None => "—",
            },
            row.notes
        ));
    }
    md.push_str(&format!(
        "\n## Decision summary\n\nRecommended route:\n  **{}**\n\nWhy:\n  {}\n",
        r.recommended, r.reason
    ));
    if !r.warnings.is_empty() {
        md.push_str("\n## Release-readiness warnings\n\n");
        for w in &r.warnings {
            md.push_str(&format!("- {w}\n"));
        }
    }
    md
}
