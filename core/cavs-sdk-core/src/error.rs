//! The SDK error model: every failure maps to a stable `CAVS-E-*` code so
//! the Go/Java/Node SDKs can surface typed errors without parsing prose.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("invalid request JSON: {0}")]
    InvalidJson(String),
    #[error("unknown operation '{0}'")]
    UnknownOperation(String),
    #[error("unsupported schema version '{0}' (this engine speaks 1.x)")]
    UnsupportedSchema(String),
    #[error("{} does not exist", .0.display())]
    PathNotFound(PathBuf),
    #[error("unsafe path in tree: {0}")]
    PathTraversal(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Plan(#[from] cavs_plan::PlanError),
    #[error(transparent)]
    Signature(#[from] cavs_signature::SignatureError),
    #[error(transparent)]
    Format(#[from] cavs_format::FormatError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Internal(String),
}

impl SdkError {
    pub fn code(&self) -> &'static str {
        match self {
            SdkError::InvalidRequest(_) => "CAVS-E-INVALID-REQUEST",
            SdkError::InvalidJson(_) => "CAVS-E-INVALID-JSON",
            SdkError::UnknownOperation(_) => "CAVS-E-UNKNOWN-OPERATION",
            SdkError::UnsupportedSchema(_) => "CAVS-E-UNSUPPORTED-SCHEMA",
            SdkError::PathNotFound(_) => "CAVS-E-PATH-NOT-FOUND",
            SdkError::PathTraversal(_) => "CAVS-E-PATH-TRAVERSAL",
            SdkError::Cancelled => "CAVS-E-CANCELLED",
            SdkError::Plan(_) => "CAVS-E-PLAN",
            SdkError::Signature(_) => "CAVS-E-SIGNATURE",
            SdkError::Format(_) => "CAVS-E-FORMAT",
            SdkError::Io(_) => "CAVS-E-IO",
            SdkError::Internal(_) => "CAVS-E-INTERNAL",
        }
    }

    /// Whether retrying the same request could succeed (transient causes).
    pub fn recoverable(&self) -> bool {
        matches!(self, SdkError::Io(_) | SdkError::Cancelled)
    }
}

impl From<anyhow::Error> for SdkError {
    fn from(e: anyhow::Error) -> Self {
        SdkError::Internal(format!("{e:#}"))
    }
}

pub type Result<T> = std::result::Result<T, SdkError>;
