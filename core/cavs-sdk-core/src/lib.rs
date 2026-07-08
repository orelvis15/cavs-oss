//! # cavs-sdk-core
//!
//! The high-level operation engine shared by the CAVS CLI-adjacent tooling,
//! the C ABI (`cavs-ffi`) and the language SDKs (Go/Java/Node). It exposes
//! a coarse, JSON-in/JSON-out surface so the FFI boundary stays tiny and
//! stable while the Rust internals are free to evolve.
//!
//! ## Envelope
//!
//! Every request is a JSON object:
//!
//! ```json
//! { "schemaVersion": "1.0", "requestId": "optional", "data": { ... } }
//! ```
//!
//! and every response is:
//!
//! ```json
//! { "schemaVersion": "1.0", "ok": true, "operation": "preview",
//!   "requestId": "optional", "data": { ... } }
//! ```
//!
//! On failure `ok` is `false` and `error` carries a stable `code`, a
//! `message`, a `recoverable` flag and optional `details`.
//!
//! The MVP operations (see [`OPERATIONS`]) are: `analyze`, `packDirectory`,
//! `previewUpdate`/`compareRoutes`, `createPlan`, `applyPlan`,
//! `verifyInstall`, `benchmark` and `estimateSavings`.

mod compare;
mod error;
mod fsutil;
mod ops;
mod progress;

pub use error::{Result, SdkError};
pub use progress::{OpCtx, ProgressEvent, ProgressSink};

use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

/// SDK/engine version (tracks the workspace crate version).
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable C ABI / JSON schema contract version. Bump the major only on a
/// breaking change to the envelope or operation semantics.
pub const ABI_VERSION: &str = "1.0.0";

/// The JSON schema version this engine speaks. Requests carrying an
/// unsupported *major* are rejected.
pub const SCHEMA_VERSION: &str = "1.0";

/// Operations the engine understands. Aliases (`previewUpdate` /
/// `compareRoutes`) map to the same implementation.
pub const OPERATIONS: &[&str] = &[
    "analyze",
    "packDirectory",
    "previewUpdate",
    "compareRoutes",
    "createPlan",
    "applyPlan",
    "verifyInstall",
    "benchmark",
    "estimateSavings",
];

/// Capability descriptor, returned by [`capabilities_json`] and the FFI's
/// `cavs_sdk_capabilities_json`.
pub fn capabilities() -> Value {
    json!({
        "abiVersion": ABI_VERSION,
        "sdkVersion": SDK_VERSION,
        "schemaVersion": SCHEMA_VERSION,
        "features": OPERATIONS,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
    })
}

pub fn capabilities_json() -> String {
    capabilities().to_string()
}

fn schema_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// Dispatch one operation given the already-parsed request `data` object.
/// This is the single place the CLI, tests and FFI funnel through.
pub fn dispatch(
    operation: &str,
    data: &Value,
    progress: Option<ProgressSink>,
    cancel: Option<&AtomicBool>,
) -> Result<Value> {
    let ctx = OpCtx::new(operation, progress, cancel);
    ctx.emit(ctx.event("started"));
    let result = match operation {
        "analyze" => ops::analyze::run(&ctx, data),
        "packDirectory" => ops::pack::run(&ctx, data),
        "previewUpdate" | "compareRoutes" => ops::preview::run(&ctx, data),
        "createPlan" => ops::plan::run(&ctx, data),
        "applyPlan" => ops::apply::run(&ctx, data),
        "verifyInstall" => ops::verify::run(&ctx, data),
        "benchmark" => ops::benchmark::run(&ctx, data),
        "estimateSavings" => ops::savings::run(&ctx, data),
        other => Err(SdkError::UnknownOperation(other.to_string())),
    };
    match &result {
        Ok(_) => ctx.emit(ctx.event("completed")),
        Err(_) => ctx.emit(ctx.event("failed")),
    }
    result
}

/// Execute a full request envelope (JSON string in, JSON string out). This
/// never returns `Err`: engine errors are encoded into the response
/// envelope so the FFI has a single success path.
pub fn execute_envelope(
    operation: &str,
    request_json: &str,
    progress: Option<ProgressSink>,
    cancel: Option<&AtomicBool>,
) -> String {
    let request_id = extract_request_id(request_json);
    match parse_envelope(request_json) {
        Ok(data) => match dispatch(operation, &data, progress, cancel) {
            Ok(value) => success_envelope(operation, request_id.as_deref(), value),
            Err(e) => error_envelope(operation, request_id.as_deref(), &e),
        },
        Err(e) => error_envelope(operation, request_id.as_deref(), &e),
    }
}

fn parse_envelope(request_json: &str) -> Result<Value> {
    let root: Value =
        serde_json::from_str(request_json).map_err(|e| SdkError::InvalidJson(e.to_string()))?;
    if let Some(version) = root.get("schemaVersion").and_then(|v| v.as_str()) {
        match schema_major(version) {
            Some(1) => {}
            _ => return Err(SdkError::UnsupportedSchema(version.to_string())),
        }
    }
    // The operation-specific payload lives under `data`; allow a bare object
    // (no envelope) as a convenience for embedders and tests.
    Ok(match root.get("data") {
        Some(data) => data.clone(),
        None => root,
    })
}

fn extract_request_id(request_json: &str) -> Option<String> {
    serde_json::from_str::<Value>(request_json)
        .ok()?
        .get("requestId")?
        .as_str()
        .map(str::to_string)
}

fn success_envelope(operation: &str, request_id: Option<&str>, data: Value) -> String {
    let mut env = json!({
        "schemaVersion": SCHEMA_VERSION,
        "ok": true,
        "operation": operation,
        "data": data,
    });
    if let Some(id) = request_id {
        env["requestId"] = json!(id);
    }
    env.to_string()
}

fn error_envelope(operation: &str, request_id: Option<&str>, err: &SdkError) -> String {
    let mut env = json!({
        "schemaVersion": SCHEMA_VERSION,
        "ok": false,
        "operation": operation,
        "error": {
            "code": err.code(),
            "message": err.to_string(),
            "recoverable": err.recoverable(),
            "details": {},
        },
    });
    if let Some(id) = request_id {
        env["requestId"] = json!(id);
    }
    env.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_semver() {
        assert_eq!(SDK_VERSION.split('.').count(), 3);
        assert_eq!(ABI_VERSION.split('.').count(), 3);
    }

    #[test]
    fn capabilities_lists_all_operations() {
        let caps = capabilities();
        let features = caps["features"].as_array().unwrap();
        assert_eq!(features.len(), OPERATIONS.len());
        assert!(features.iter().any(|f| f == "createPlan"));
    }

    #[test]
    fn unknown_operation_is_structured_error() {
        let out = execute_envelope("bogus", "{}", None, None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "CAVS-E-UNKNOWN-OPERATION");
    }

    #[test]
    fn invalid_json_is_structured_error() {
        let out = execute_envelope("analyze", "{not json", None, None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "CAVS-E-INVALID-JSON");
    }

    #[test]
    fn unsupported_schema_rejected() {
        let out = execute_envelope(
            "analyze",
            r#"{"schemaVersion":"2.0","data":{}}"#,
            None,
            None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "CAVS-E-UNSUPPORTED-SCHEMA");
    }

    #[test]
    fn request_id_echoed() {
        let out = execute_envelope(
            "estimateSavings",
            r#"{"schemaVersion":"1.0","requestId":"abc","data":{
                "pricePerGb":0.08,"monthlyDownloads":500000,
                "averageFullDownloadBytes":65011712,
                "averageCavsDownloadBytes":2631921}}"#,
            None,
            None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["requestId"], "abc");
        assert!(v["data"]["savingsPercent"].as_f64().unwrap() > 90.0);
    }

    #[test]
    fn analyze_missing_path_reports_not_found() {
        let out = execute_envelope(
            "analyze",
            r#"{"data":{"oldPath":"/no/such/old","newPath":"/no/such/new"}}"#,
            None,
            None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], "CAVS-E-PATH-NOT-FOUND");
    }
}
