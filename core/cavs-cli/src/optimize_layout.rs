//! `cavs optimize-layout` (v0.9.0): advisory recommendations to
//! restructure a build so updates get smaller. Never modifies files;
//! `--write-plan` emits machine-readable JSON for future automation.

use crate::report::human_bytes;
use anyhow::Result;
use cavs_analyzer::compare::{analyze, Analysis};
use cavs_analyzer::detect::{Severity, Thresholds};
use cavs_analyzer::Engine;
use serde::Serialize;
use std::path::Path;

pub struct LayoutArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub engine: &'a str,
    pub out: Option<&'a Path>,
    pub write_plan: Option<&'a Path>,
    pub json: bool,
}

#[derive(Serialize)]
pub struct LayoutRecommendation {
    pub id: u32,
    pub severity: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub current: String,
    pub suggested: String,
    /// SteamPipe-style estimate today.
    pub estimated_update_now: u64,
    /// What the update could approach after the fix (content-defined
    /// estimate as the proxy for a layout-friendly build).
    pub estimated_update_after: u64,
}

#[derive(Serialize)]
pub struct LayoutPlan {
    pub old: String,
    pub new: String,
    pub engine: String,
    pub recommendations: Vec<LayoutRecommendation>,
    /// General patch-friendly layout rules that always apply.
    pub general_rules: Vec<String>,
    pub note: String,
}

pub const GENERAL_RULES: &[&str] = &[
    "Split oversized packs (keep them under ~2 GiB).",
    "Group assets by level/feature so updates stay local.",
    "Keep asset ordering stable between builds.",
    "Add new content as new packs; keep released packs immutable.",
    "Avoid timestamps, build IDs and generated names inside packs.",
    "Move the TOC to the beginning or end of the pack; prefer relative offsets.",
    "Use per-asset compression instead of global pack compression.",
    "Align Unreal-style pack padding to 1 MiB when targeting fixed-chunk updaters.",
    "Separate platform-specific binaries from shared data (they patch differently).",
];

fn suggestion_for(kind: &str, file: Option<&str>) -> (String, String) {
    let f = file.unwrap_or("the pack");
    match kind {
        "oversized_pack" => {
            let stem = f.rsplit_once('.').map(|(s, _)| s).unwrap_or(f);
            (
                format!("{f} is a monolithic pack"),
                format!(
                    "split {f} into per-level/per-feature parts, e.g. \
                     `{stem}_level_01`, `{stem}_level_02`, `{stem}_shared`"
                ),
            )
        }
        "scattered_pack_churn" => (
            format!("{f} changes all over the file every release"),
            "group assets by update cadence and split the pack by level/feature".into(),
        ),
        "asset_shuffling" => (
            format!("{f} keeps its content but shuffles offsets"),
            "make packing deterministic: stable asset order, stable padding".into(),
        ),
        "toc_churn" => (
            format!("{f} rewrites distributed TOC/offset entries"),
            "centralize the TOC at the start/end and use relative offsets".into(),
        ),
        "compressed_blob" => (
            format!("{f} is compressed as one stream"),
            "compress per asset so a change only dirties that asset".into(),
        ),
        "metadata_churn" => (
            "many files change only in embedded metadata".into(),
            "strip or pin timestamps/build IDs at export time".into(),
        ),
        "new_content_in_old_pack" => (
            format!("new content was packed into {f}"),
            "ship new levels/features as new pack files".into(),
        ),
        _ => ("see analyzer finding".into(), "see analyzer finding".into()),
    }
}

pub fn plan_from_analysis(a: &Analysis) -> LayoutPlan {
    let mut recommendations = Vec::new();
    let mut id = 0u32;
    for f in &a.findings {
        if f.kind == "engine_hint" {
            continue;
        }
        id += 1;
        let (current, suggested) = suggestion_for(&f.kind, f.file.as_deref());
        let (now, after) = match &f.file {
            Some(path) => a
                .files
                .iter()
                .find(|x| &x.path == path)
                .map(|x| (x.steam_download, x.cdc_download))
                .unwrap_or((f.estimated_wasted_bytes, 0)),
            None => (f.estimated_wasted_bytes, 0),
        };
        recommendations.push(LayoutRecommendation {
            id,
            severity: f.severity.label().into(),
            title: f.title.clone(),
            file: f.file.clone(),
            current,
            suggested,
            estimated_update_now: now,
            estimated_update_after: after,
        });
    }
    LayoutPlan {
        old: a.old_build.clone(),
        new: a.new_build.clone(),
        engine: a.engine.clone(),
        recommendations,
        general_rules: GENERAL_RULES.iter().map(|s| s.to_string()).collect(),
        note: format!(
            "Advisory only — no files were modified. {}",
            cavs_analyzer::ESTIMATE_NOTE
        ),
    }
}

pub fn optimize_layout(args: &LayoutArgs) -> Result<()> {
    let analysis = analyze(
        args.old,
        args.new,
        Engine::parse(args.engine),
        &Thresholds::default(),
        &|_: &str| true,
    )?;
    let plan = plan_from_analysis(&analysis);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!("optimize-layout: {} → {}", plan.old, plan.new);
        if plan.recommendations.is_empty() {
            println!("  no layout problems detected — the build already patches well");
        }
        for r in &plan.recommendations {
            println!(
                "  {}. [{}] {}{}",
                r.id,
                r.severity,
                r.title,
                r.file
                    .as_deref()
                    .map(|f| format!(" — {f}"))
                    .unwrap_or_default()
            );
            println!(
                "     now ~{} → after fix ~{}",
                human_bytes(r.estimated_update_now),
                human_bytes(r.estimated_update_after)
            );
        }
        println!("note    : {}", plan.note);
    }
    if let Some(path) = args.out {
        std::fs::write(path, markdown(&plan))?;
        eprintln!("report  : {}", path.display());
    }
    if let Some(path) = args.write_plan {
        std::fs::write(path, serde_json::to_vec_pretty(&plan)?)?;
        eprintln!("plan    : {}", path.display());
    }
    let critical = analysis
        .findings
        .iter()
        .any(|f| f.severity == Severity::Critical);
    if critical {
        eprintln!("status  : critical layout issues found");
    }
    Ok(())
}

fn markdown(p: &LayoutPlan) -> String {
    let mut md = String::from("# Layout Optimization Plan\n\n");
    md.push_str(&format!("> {}\n\n", p.note));
    md.push_str(&format!(
        "`{}` → `{}` (engine: {})\n",
        p.old, p.new, p.engine
    ));
    for r in &p.recommendations {
        md.push_str(&format!("\n## Recommendation {}: {}\n\n", r.id, r.title));
        if let Some(f) = &r.file {
            md.push_str(&format!("File: `{f}`\n\n"));
        }
        md.push_str(&format!("Current:\n  {}\n\n", r.current));
        md.push_str(&format!("Suggested:\n  {}\n\n", r.suggested));
        md.push_str(&format!(
            "Expected:\n  estimated SteamPipe-style update {} → ~{}\n",
            human_bytes(r.estimated_update_now),
            human_bytes(r.estimated_update_after)
        ));
    }
    if p.recommendations.is_empty() {
        md.push_str("\nNo layout problems detected — the build already patches well.\n");
    }
    md.push_str("\n## General patch-friendly layout rules\n\n");
    for rule in &p.general_rules {
        md.push_str(&format!("- {rule}\n"));
    }
    md
}
