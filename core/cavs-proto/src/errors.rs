//! Structured error taxonomy (v0.5.0 production hardening).
//!
//! Stable, machine-readable error codes shared by the CLI, server, client
//! and plugin so callers can decide (retry, repair, give up) without
//! parsing prose. The codes are a public contract: once released they
//! never change meaning, only new ones are added.
//!
//! Errors keep flowing through each binary's error type (`anyhow`,
//! `FormatError`, …); the code travels as a `CAVS-E-*` prefix on the
//! message, so it survives context wrapping and shows up in logs, stderr
//! and JSON reports unchanged. [`error_code_of`] recovers the code from
//! any rendered error chain.

/// Every stable CAVS error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The manifest bytes are unparseable or fail integrity checks.
    ManifestCorrupt,
    /// The manifest parsed but declares an unsupported format version.
    UnsupportedManifestVersion,
    /// A `.cavs` container is unparseable or fails integrity checks.
    ContainerCorrupt,
    /// The bootstrap artifact does not match its announced BLAKE3.
    BootstrapHashMismatch,
    /// A chunk's bytes do not match its content hash.
    ChunkHashMismatch,
    /// A packfile or pack index is unparseable or fails integrity checks.
    PackCorrupt,
    /// A cache entry is corrupt but the cache can recover (re-fetch).
    CacheCorruptRecoverable,
    /// The reconstructed output does not match the manifest's digest.
    OutputHashMismatch,
    /// An Ed25519 content signature is present but invalid or untrusted.
    SignatureInvalid,
    /// A network operation failed after exhausting retries.
    Network,
    /// A resume journal exists but no longer matches the requested fetch.
    ResumeInvalid,
    /// Not enough disk space to complete the operation.
    DiskFull,
    /// The input requires a feature this build does not support.
    UnsupportedFeature,
    /// A `.cavssig` signature file is unparseable or fails integrity checks.
    SignatureCorrupt,
    /// A signature parsed but does not describe the given source artifact.
    SignatureMismatch,
    /// `--previous-artifact` points to a file that does not exist.
    PreviousArtifactMissing,
    /// A previous-artifact range failed verification; the client falls back
    /// to cache/network for that range (recoverable).
    PreviousArtifactMismatch,
    /// A hybrid reconstruction plan is internally inconsistent (gaps or
    /// overlaps in output coverage).
    HybridPlanInvalid,
    /// A hybrid source failed mid-execution and no fallback succeeded.
    HybridSourceFailed,
    /// A directory/container apply failed; the previous install is intact.
    ContainerApplyFailed,
    /// A directory/container rollback could not restore the previous state.
    ContainerRollbackFailed,
    /// `cavs bench wharf` needs an external tool that is not available.
    WharfBenchUnavailable,
}

impl ErrorCode {
    /// The stable wire/log representation, e.g. `CAVS-E-MANIFEST-CORRUPT`.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::ManifestCorrupt => "CAVS-E-MANIFEST-CORRUPT",
            ErrorCode::UnsupportedManifestVersion => "CAVS-E-UNSUPPORTED-MANIFEST-VERSION",
            ErrorCode::ContainerCorrupt => "CAVS-E-CONTAINER-CORRUPT",
            ErrorCode::BootstrapHashMismatch => "CAVS-E-BOOTSTRAP-HASH-MISMATCH",
            ErrorCode::ChunkHashMismatch => "CAVS-E-CHUNK-HASH-MISMATCH",
            ErrorCode::PackCorrupt => "CAVS-E-PACK-CORRUPT",
            ErrorCode::CacheCorruptRecoverable => "CAVS-E-CACHE-CORRUPT-RECOVERABLE",
            ErrorCode::OutputHashMismatch => "CAVS-E-OUTPUT-HASH-MISMATCH",
            ErrorCode::SignatureInvalid => "CAVS-E-SIGNATURE-INVALID",
            ErrorCode::Network => "CAVS-E-NETWORK",
            ErrorCode::ResumeInvalid => "CAVS-E-RESUME-INVALID",
            ErrorCode::DiskFull => "CAVS-E-DISK-FULL",
            ErrorCode::UnsupportedFeature => "CAVS-E-UNSUPPORTED-FEATURE",
            ErrorCode::SignatureCorrupt => "CAVS-E-SIGNATURE-CORRUPT",
            ErrorCode::SignatureMismatch => "CAVS-E-SIGNATURE-MISMATCH",
            ErrorCode::PreviousArtifactMissing => "CAVS-E-PREVIOUS-ARTIFACT-MISSING",
            ErrorCode::PreviousArtifactMismatch => "CAVS-E-PREVIOUS-ARTIFACT-MISMATCH",
            ErrorCode::HybridPlanInvalid => "CAVS-E-HYBRID-PLAN-INVALID",
            ErrorCode::HybridSourceFailed => "CAVS-E-HYBRID-SOURCE-FAILED",
            ErrorCode::ContainerApplyFailed => "CAVS-E-CONTAINER-APPLY-FAILED",
            ErrorCode::ContainerRollbackFailed => "CAVS-E-CONTAINER-ROLLBACK-FAILED",
            ErrorCode::WharfBenchUnavailable => "CAVS-E-WHARF-BENCH-UNAVAILABLE",
        }
    }

    /// Whether an operation that failed with this code is worth retrying
    /// unchanged (transient failure) or requires a different action.
    pub fn is_retryable(self) -> bool {
        matches!(self, ErrorCode::Network)
    }

    /// Whether the client can recover from this failure inside the same
    /// operation by switching source (e.g. a corrupt previous artifact
    /// falls back to cache/network).
    pub fn is_recoverable(self) -> bool {
        matches!(
            self,
            ErrorCode::Network
                | ErrorCode::CacheCorruptRecoverable
                | ErrorCode::PreviousArtifactMissing
                | ErrorCode::PreviousArtifactMismatch
        )
    }

    /// Prefix `msg` with the code: the canonical error-message shape.
    pub fn msg(self, msg: impl std::fmt::Display) -> String {
        format!("{}: {msg}", self.as_str())
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// All codes, for docs/tests.
pub const ALL_ERROR_CODES: [ErrorCode; 22] = [
    ErrorCode::ManifestCorrupt,
    ErrorCode::UnsupportedManifestVersion,
    ErrorCode::ContainerCorrupt,
    ErrorCode::BootstrapHashMismatch,
    ErrorCode::ChunkHashMismatch,
    ErrorCode::PackCorrupt,
    ErrorCode::CacheCorruptRecoverable,
    ErrorCode::OutputHashMismatch,
    ErrorCode::SignatureInvalid,
    ErrorCode::Network,
    ErrorCode::ResumeInvalid,
    ErrorCode::DiskFull,
    ErrorCode::UnsupportedFeature,
    ErrorCode::SignatureCorrupt,
    ErrorCode::SignatureMismatch,
    ErrorCode::PreviousArtifactMissing,
    ErrorCode::PreviousArtifactMismatch,
    ErrorCode::HybridPlanInvalid,
    ErrorCode::HybridSourceFailed,
    ErrorCode::ContainerApplyFailed,
    ErrorCode::ContainerRollbackFailed,
    ErrorCode::WharfBenchUnavailable,
];

/// Recover the first `CAVS-E-*` code embedded in a rendered error message
/// (or error chain rendered with `{:#}`/`{:?}`).
pub fn error_code_of(rendered: &str) -> Option<ErrorCode> {
    let pos = rendered.find("CAVS-E-")?;
    let tail = &rendered[pos..];
    ALL_ERROR_CODES
        .into_iter()
        .filter(|c| tail.starts_with(c.as_str()))
        // Longest match wins: CAVS-E-CACHE-CORRUPT-RECOVERABLE must not be
        // shadowed by a shorter code that happens to be its prefix.
        .max_by_key(|c| c.as_str().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_recoverable_from_messages() {
        for code in ALL_ERROR_CODES {
            assert!(code.as_str().starts_with("CAVS-E-"));
            let msg = code.msg("something went wrong");
            assert_eq!(error_code_of(&msg), Some(code));
            let wrapped = format!("fetch failed: caused by: {msg} (attempt 3)");
            assert_eq!(error_code_of(&wrapped), Some(code));
        }
        assert_eq!(error_code_of("plain error, no code"), None);
    }

    #[test]
    fn codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for code in ALL_ERROR_CODES {
            assert!(seen.insert(code.as_str()), "duplicate {}", code.as_str());
        }
    }
}
