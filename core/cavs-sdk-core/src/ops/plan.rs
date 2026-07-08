//! `createPlan` — build a portable `.cavsplan` from an old build (or its
//! signature) and a new build, via `cavs-plan`.

use crate::error::{Result, SdkError};
use crate::progress::OpCtx;
use cavs_plan::{BuildOptions, PlanKind};
use cavs_signature::CavsSignature;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

pub const DEFAULT_BLOCK_KIB: u32 = 64;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePlanRequest {
    /// Old build path (file or directory). Optional if `oldSignature` given.
    #[serde(default)]
    old_path: Option<PathBuf>,
    /// Precomputed `.cavssig` for the old build.
    #[serde(default)]
    old_signature: Option<PathBuf>,
    new_path: PathBuf,
    output_plan: PathBuf,
    /// "portable" (includes payload) or "analysis" (estimates only).
    #[serde(default = "default_kind")]
    plan_kind: String,
    #[serde(default = "default_block_kib")]
    block_kib: u32,
    #[serde(default = "default_zstd")]
    zstd_level: i32,
}

fn default_kind() -> String {
    "portable".to_string()
}

fn default_block_kib() -> u32 {
    DEFAULT_BLOCK_KIB
}

fn default_zstd() -> i32 {
    19
}

/// Load the old signature from a `.cavssig` if given, else sign the old
/// build on the fly. Shared with the preview/benchmark ops.
pub fn old_signature(
    old_path: Option<&std::path::Path>,
    old_signature: Option<&std::path::Path>,
    block_kib: u32,
) -> Result<CavsSignature> {
    match (old_signature, old_path) {
        (Some(sig_path), _) => {
            if !sig_path.exists() {
                return Err(SdkError::PathNotFound(sig_path.to_path_buf()));
            }
            let bytes = std::fs::read(sig_path)?;
            Ok(CavsSignature::decode(&bytes)?)
        }
        (None, Some(old)) => {
            if !old.exists() {
                return Err(SdkError::PathNotFound(old.to_path_buf()));
            }
            let block_size = block_kib.saturating_mul(1024).max(1024);
            let label = old
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if old.is_dir() {
                Ok(CavsSignature::sign_dir(old, block_size, &label)?)
            } else {
                Ok(CavsSignature::sign_file(old, block_size, &label)?)
            }
        }
        (None, None) => Err(SdkError::InvalidRequest(
            "provide oldPath or oldSignature".to_string(),
        )),
    }
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let started = std::time::Instant::now();
    let req: CreatePlanRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    if !req.new_path.exists() {
        return Err(SdkError::PathNotFound(req.new_path.clone()));
    }
    let kind = match req.plan_kind.as_str() {
        "portable" => PlanKind::Portable,
        "analysis" => PlanKind::Analysis,
        other => {
            return Err(SdkError::InvalidRequest(format!(
                "unknown planKind '{other}' (expected portable or analysis)"
            )))
        }
    };

    ctx.phase("signing");
    ctx.check_cancelled()?;
    let sig = old_signature(
        req.old_path.as_deref(),
        req.old_signature.as_deref(),
        req.block_kib,
    )?;

    ctx.phase("diffing");
    ctx.check_cancelled()?;
    let plan = cavs_plan::build(
        &sig,
        &req.new_path,
        &BuildOptions {
            kind,
            zstd_level: req.zstd_level,
        },
    )?;
    let summary = plan.summary();

    ctx.phase("encoding");
    let encoded = plan.encode(req.zstd_level);
    std::fs::write(&req.output_plan, &encoded)?;

    Ok(json!({
        "planPath": req.output_plan,
        "planBytes": encoded.len() as u64,
        "planKind": kind.label(),
        "mode": plan.mode.label(),
        "operationCount": summary.ops_total,
        "copyOps": summary.copy_ops,
        "inlineOps": summary.inline_ops,
        "reusedBytes": summary.reused_bytes,
        "inlineBytes": summary.inline_bytes,
        "estimatedNetworkBytes": encoded.len() as u64,
        "expectedOutputSize": plan.new_size,
        "files": summary.files,
        "unchangedFiles": summary.unchanged_files,
        "deleted": summary.deleted,
        "elapsedMs": started.elapsed().as_millis() as u64,
    }))
}
