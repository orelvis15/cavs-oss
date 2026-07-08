//! `previewUpdate` / `compareRoutes` — estimate the wire cost of shipping
//! `newPath` to a client that already has `oldPath`, across the delivery
//! routes CAVS models, and recommend the cheapest by network bytes.
//!
//! Route sources:
//! - `fullRaw` / `steamPipeStyle` / `cavsChunk` come from the analyzer's
//!   old→new model (fixed 1 MiB same-path reuse vs FastCDC global reuse);
//! - `cavsPlan` is the exact encoded size of a portable `.cavsplan`.

use crate::error::{Result, SdkError};
use crate::ops::plan::{old_signature, DEFAULT_BLOCK_KIB};
use crate::progress::OpCtx;
use cavs_analyzer::detect::Thresholds;
use cavs_analyzer::Engine;
use cavs_plan::{BuildOptions, PlanKind};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewRequest {
    old_path: PathBuf,
    new_path: PathBuf,
    #[serde(default = "default_engine")]
    engine_hint: String,
    /// Which routes to include; empty/absent = all modeled routes.
    #[serde(default)]
    routes: Vec<String>,
}

fn default_engine() -> String {
    "auto".to_string()
}

struct Route {
    name: &'static str,
    network_bytes: u64,
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let req: PreviewRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    for p in [&req.old_path, &req.new_path] {
        if !p.exists() {
            return Err(SdkError::PathNotFound(p.clone()));
        }
    }

    ctx.phase("analyzing");
    ctx.check_cancelled()?;
    let analysis = cavs_analyzer::compare::analyze(
        &req.old_path,
        &req.new_path,
        Engine::parse(&req.engine_hint),
        &Thresholds::default(),
        &|_| true,
    )?;

    ctx.phase("planning");
    ctx.check_cancelled()?;
    let sig = old_signature(Some(&req.old_path), None, DEFAULT_BLOCK_KIB)?;
    let plan = cavs_plan::build(
        &sig,
        &req.new_path,
        &BuildOptions {
            kind: PlanKind::Portable,
            zstd_level: 19,
        },
    )?;
    let cavs_plan_bytes = plan.encode(19).len() as u64;

    let mut all = vec![
        Route {
            name: "fullRaw",
            network_bytes: analysis.new_size_bytes,
        },
        Route {
            name: "steamPipeStyle",
            network_bytes: analysis.estimated_steampipe_download,
        },
        Route {
            name: "cavsChunk",
            network_bytes: analysis.estimated_cavs_download,
        },
        Route {
            name: "cavsPlan",
            network_bytes: cavs_plan_bytes,
        },
    ];

    if !req.routes.is_empty() {
        let wanted: std::collections::HashSet<&str> =
            req.routes.iter().map(|s| s.as_str()).collect();
        all.retain(|r| wanted.contains(r.name));
        if all.is_empty() {
            return Err(SdkError::InvalidRequest(
                "no known routes selected".to_string(),
            ));
        }
    }

    let recommended = all
        .iter()
        .min_by_key(|r| r.network_bytes)
        .map(|r| r.name)
        .unwrap_or("cavsPlan");

    let routes: Vec<Value> = all
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "networkBytes": r.network_bytes,
                "available": true,
            })
        })
        .collect();

    Ok(json!({
        "recommendedRoute": recommended,
        "oldSizeBytes": analysis.old_size_bytes,
        "newSizeBytes": analysis.new_size_bytes,
        "routes": routes,
        "explanation": format!(
            "{recommended} ships the fewest bytes for this old→new transition."
        ),
    }))
}
