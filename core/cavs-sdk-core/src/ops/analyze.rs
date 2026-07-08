//! `analyze` — inspect an old→new build transition and explain update
//! behavior (sizes, per-file cost, findings), via `cavs-analyzer`.

use crate::error::{Result, SdkError};
use crate::progress::OpCtx;
use cavs_analyzer::detect::{Severity, Thresholds};
use cavs_analyzer::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeRequest {
    old_path: PathBuf,
    new_path: PathBuf,
    #[serde(default = "default_engine")]
    engine_hint: String,
    #[serde(default = "default_worst")]
    max_worst_files: usize,
}

fn default_engine() -> String {
    "auto".to_string()
}

fn default_worst() -> usize {
    10
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let req: AnalyzeRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    for p in [&req.old_path, &req.new_path] {
        if !p.exists() {
            return Err(SdkError::PathNotFound(p.clone()));
        }
    }
    ctx.check_cancelled()?;
    ctx.phase("analyzing");

    let analysis = cavs_analyzer::compare::analyze(
        &req.old_path,
        &req.new_path,
        Engine::parse(&req.engine_hint),
        &Thresholds::default(),
        &|_| true,
    )?;
    ctx.check_cancelled()?;

    let worst: Vec<Value> = analysis
        .files
        .iter()
        .take(req.max_worst_files)
        .map(|f| {
            json!({
                "path": f.path,
                "status": f.status,
                "isPack": f.is_pack,
                "oldSizeBytes": f.old_size,
                "newSizeBytes": f.new_size,
                "estimatedDownloadBytes": f.cdc_download,
                "steamPipeDownloadBytes": f.steam_download,
                "reuseRatio": f.cdc_reuse_ratio,
                "entropyBits": f.entropy_bits,
            })
        })
        .collect();

    let warnings: Vec<String> = analysis
        .findings
        .iter()
        .filter(|f| f.severity >= Severity::Warning)
        .map(|f| f.title.clone())
        .collect();
    let recommendations: Vec<Value> = analysis
        .findings
        .iter()
        .map(|f| {
            json!({
                "severity": f.severity.label(),
                "kind": f.kind,
                "title": f.title,
                "file": f.file,
                "estimatedWastedBytes": f.estimated_wasted_bytes,
                "why": f.why,
                "fix": f.fix,
                "expectedImprovement": f.expected_improvement,
            })
        })
        .collect();

    Ok(json!({
        "summary": {
            "oldSizeBytes": analysis.old_size_bytes,
            "newSizeBytes": analysis.new_size_bytes,
            "estimatedUpdateBytes": analysis.estimated_cavs_download,
            "estimatedSteamPipeBytes": analysis.estimated_steampipe_download,
            "cavsReuseRatio": analysis.cdc_reuse_ratio,
            "steamPipeReuseRatio": analysis.steam_reuse_ratio,
            "filesUnchanged": analysis.files_unchanged,
            "filesModified": analysis.files_modified,
            "filesAdded": analysis.files_added,
            "filesDeleted": analysis.files_deleted,
            "worstFiles": worst,
        },
        "engine": analysis.engine,
        "warnings": warnings,
        "recommendations": recommendations,
        "note": analysis.note,
    }))
}
