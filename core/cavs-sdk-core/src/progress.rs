//! Progress events and the per-operation context handed to every op:
//! a progress sink (optional) and a cooperative cancellation flag.

use crate::error::{Result, SdkError};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

/// One progress event, serialized as camelCase JSON across the FFI.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub type ProgressSink<'a> = &'a (dyn Fn(&ProgressEvent) + Send + Sync);

/// Execution context for one operation.
pub struct OpCtx<'a> {
    pub operation: &'a str,
    pub progress: Option<ProgressSink<'a>>,
    pub cancel: Option<&'a AtomicBool>,
}

impl<'a> OpCtx<'a> {
    pub fn new(
        operation: &'a str,
        progress: Option<ProgressSink<'a>>,
        cancel: Option<&'a AtomicBool>,
    ) -> Self {
        OpCtx {
            operation,
            progress,
            cancel,
        }
    }

    pub fn emit(&self, event: ProgressEvent) {
        if let Some(sink) = self.progress {
            sink(&event);
        }
    }

    pub fn event(&self, kind: &str) -> ProgressEvent {
        ProgressEvent {
            kind: kind.to_string(),
            operation: self.operation.to_string(),
            phase: None,
            current_bytes: None,
            total_bytes: None,
            percentage: None,
            message: None,
        }
    }

    pub fn phase(&self, phase: &str) {
        let mut e = self.event("phaseChanged");
        e.phase = Some(phase.to_string());
        self.emit(e);
    }

    pub fn bytes(&self, phase: &str, current: u64, total: u64, message: Option<String>) {
        let mut e = self.event("progress");
        e.phase = Some(phase.to_string());
        e.current_bytes = Some(current);
        e.total_bytes = Some(total);
        e.percentage = Some(if total == 0 {
            1.0
        } else {
            current as f64 / total as f64
        });
        e.message = message;
        self.emit(e);
    }

    /// Bail out with `CAVS-E-CANCELLED` if the caller has asked us to stop.
    pub fn check_cancelled(&self) -> Result<()> {
        match self.cancel {
            Some(flag) if flag.load(Ordering::Relaxed) => Err(SdkError::Cancelled),
            _ => Ok(()),
        }
    }
}
