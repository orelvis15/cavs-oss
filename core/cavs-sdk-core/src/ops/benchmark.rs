//! `benchmark` — a repeatable route-comparison report for CI/CD. It reuses
//! the preview route model and adds measured diff/apply timings for the
//! CAVS plan route so pipelines can gate on real numbers.

use crate::error::{Result, SdkError};
use crate::ops::plan::{old_signature, DEFAULT_BLOCK_KIB};
use crate::progress::OpCtx;
use cavs_analyzer::detect::Thresholds;
use cavs_analyzer::Engine;
use cavs_plan::apply::apply_artifact;
use cavs_plan::{BuildOptions, OfflinePlan, PlanKind, PlanMode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkRequest {
    old_path: PathBuf,
    new_path: PathBuf,
    #[serde(default = "default_engine")]
    engine_hint: String,
    /// Measure the CAVS plan apply into a temp dir (adds a full apply pass).
    #[serde(default = "default_true")]
    measure_apply: bool,
}

fn default_engine() -> String {
    "auto".to_string()
}

fn default_true() -> bool {
    true
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let req: BenchmarkRequest = serde_json::from_value(request.clone())
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
    let diff_started = std::time::Instant::now();
    let sig = old_signature(Some(&req.old_path), None, DEFAULT_BLOCK_KIB)?;
    let plan = cavs_plan::build(
        &sig,
        &req.new_path,
        &BuildOptions {
            kind: PlanKind::Portable,
            zstd_level: 19,
        },
    )?;
    let encoded = plan.encode(19);
    let diff_ms = diff_started.elapsed().as_millis() as u64;

    let mut apply_ms = None;
    if req.measure_apply && plan.mode == PlanMode::Artifact {
        ctx.phase("applying");
        ctx.check_cancelled()?;
        let tmp = TempDir::new()?;
        let out = tmp.path().join("out");
        let reloaded = OfflinePlan::decode(&encoded)?;
        let started = std::time::Instant::now();
        apply_artifact(&reloaded, &req.old_path, &out)?;
        apply_ms = Some(started.elapsed().as_millis() as u64);
    }

    let routes = json!([
        {
            "name": "fullRaw",
            "networkBytes": analysis.new_size_bytes,
        },
        {
            "name": "steamPipeStyle",
            "networkBytes": analysis.estimated_steampipe_download,
        },
        {
            "name": "cavsChunk",
            "networkBytes": analysis.estimated_cavs_download,
        },
        {
            "name": "cavsPlan",
            "networkBytes": encoded.len() as u64,
            "diffMs": diff_ms,
            "applyMs": apply_ms,
        },
    ]);

    Ok(json!({
        "oldSizeBytes": analysis.old_size_bytes,
        "newSizeBytes": analysis.new_size_bytes,
        "recommendedRoute": "cavsPlan",
        "routes": routes,
        "reuseRatio": analysis.cdc_reuse_ratio,
    }))
}

// A tiny temp-dir helper so cavs-sdk-core does not pull `tempfile` into its
// runtime dependency set (dev-dependency only). Directory is removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<Self> {
        let base = std::env::temp_dir();
        // A monotonically-unique name without needing the `rand`/`tempfile`
        // crates at runtime: pid + a process-lifetime counter.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = base.join(format!("cavs-sdk-bench-{pid}-{n}"));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
