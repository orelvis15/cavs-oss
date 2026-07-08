//! `applyPlan` — apply a `.cavsplan` to an old build, producing the new
//! build, via `cavs-plan`'s atomic artifact / journaled directory apply.

use crate::error::{Result, SdkError};
use crate::progress::OpCtx;
use cavs_plan::apply::{apply_artifact, apply_dir, ApplyOptions};
use cavs_plan::{OfflinePlan, PlanMode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPlanRequest {
    old_path: PathBuf,
    plan_path: PathBuf,
    output_path: PathBuf,
    /// Re-hash the old source against the plan's BLAKE3 before applying.
    #[serde(default)]
    check_old: bool,
    /// Delete old paths the plan marks as removed (directory mode).
    #[serde(default)]
    delete_removed: bool,
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let req: ApplyPlanRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    if !req.old_path.exists() {
        return Err(SdkError::PathNotFound(req.old_path.clone()));
    }
    if !req.plan_path.is_file() {
        return Err(SdkError::PathNotFound(req.plan_path.clone()));
    }

    ctx.phase("loading");
    ctx.check_cancelled()?;
    let bytes = std::fs::read(&req.plan_path)?;
    let plan = OfflinePlan::decode(&bytes)?;

    ctx.phase("applying");
    ctx.check_cancelled()?;
    let stats = match plan.mode {
        PlanMode::Artifact => apply_artifact(&plan, &req.old_path, &req.output_path)?,
        PlanMode::Directory => apply_dir(
            &plan,
            &req.old_path,
            &req.output_path,
            &ApplyOptions {
                delete_removed: req.delete_removed,
                check_old: req.check_old,
                plan_path: Some(req.plan_path.clone()),
            },
        )?,
    };

    Ok(json!({
        "outputPath": req.output_path,
        "verified": true,
        "mode": plan.mode.label(),
        "filesTotal": stats.files_total,
        "filesWritten": stats.files_written,
        "filesNoop": stats.files_noop,
        "dirsCreated": stats.dirs_created,
        "symlinksCreated": stats.symlinks_created,
        "deleted": stats.deleted,
        "bytesWritten": stats.bytes_written,
        "bytesFromOld": stats.bytes_from_old,
        "bytesFromBlob": stats.bytes_from_blob,
        "elapsedMs": stats.elapsed_ms,
    }))
}
