//! `verifyInstall` — does an installed build match a known-good `.cavssig`
//! or a manifest's recorded SHA-256 digests? Mirrors `cavs verify-install`.

use crate::compare::{classify, FileState};
use crate::error::{Result, SdkError};
use crate::fsutil::walk_sorted;
use crate::progress::OpCtx;
use cavs_signature::CavsSignature;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyRequest {
    target: PathBuf,
    #[serde(default)]
    signature: Option<PathBuf>,
    #[serde(default)]
    manifest: Option<PathBuf>,
    /// Tolerate files present on disk but absent from the reference.
    #[serde(default)]
    allow_extra: bool,
}

struct Report {
    files_checked: u64,
    bytes_checked: u64,
    modified: Vec<String>,
    missing: Vec<String>,
    extra: Vec<String>,
}

pub fn run(ctx: &OpCtx, request: &Value) -> Result<Value> {
    let started = std::time::Instant::now();
    let req: VerifyRequest = serde_json::from_value(request.clone())
        .map_err(|e| SdkError::InvalidRequest(e.to_string()))?;
    if !req.target.exists() {
        return Err(SdkError::PathNotFound(req.target.clone()));
    }
    ctx.phase("verifying");
    ctx.check_cancelled()?;

    let mut report = match (req.signature.as_deref(), req.manifest.as_deref()) {
        (Some(sig), None) => verify_against_signature(&req.target, sig)?,
        (None, Some(mf)) => verify_against_manifest(&req.target, mf)?,
        _ => {
            return Err(SdkError::InvalidRequest(
                "provide exactly one of signature or manifest".to_string(),
            ))
        }
    };
    if req.allow_extra {
        report.extra.clear();
    }
    let ok = report.modified.is_empty() && report.missing.is_empty() && report.extra.is_empty();

    Ok(json!({
        "verified": ok,
        "filesChecked": report.files_checked,
        "bytesChecked": report.bytes_checked,
        "mismatches": {
            "modified": report.modified,
            "missing": report.missing,
            "extra": report.extra,
        },
        "elapsedMs": started.elapsed().as_millis() as u64,
    }))
}

fn verify_against_signature(target: &Path, sig_path: &Path) -> Result<Report> {
    if !sig_path.exists() {
        return Err(SdkError::PathNotFound(sig_path.to_path_buf()));
    }
    let sig = CavsSignature::decode(&std::fs::read(sig_path)?)?;
    let entries = classify(&sig, target)?;
    let mut report = Report {
        files_checked: 0,
        bytes_checked: 0,
        modified: Vec::new(),
        missing: Vec::new(),
        extra: Vec::new(),
    };
    for e in entries {
        match e.state {
            FileState::Same => {
                report.files_checked += 1;
                report.bytes_checked += e.size;
            }
            FileState::Modified => {
                report.files_checked += 1;
                report.bytes_checked += e.size;
                report.modified.push(e.path);
            }
            FileState::Deleted => report.missing.push(e.path),
            FileState::New => report.extra.push(e.path),
        }
    }
    Ok(report)
}

fn verify_against_manifest(target: &Path, mf_path: &Path) -> Result<Report> {
    if !mf_path.exists() {
        return Err(SdkError::PathNotFound(mf_path.to_path_buf()));
    }
    let bytes = std::fs::read(mf_path)?;
    let loaded = cavs_manifest::read_manifest(&bytes)
        .map_err(|e| SdkError::Internal(format!("manifest corrupt: {e}")))?;
    let digests: Vec<(String, String)> = loaded
        .manifest
        .meta
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("sha256:")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();
    if digests.is_empty() {
        return Err(SdkError::InvalidRequest(format!(
            "{} records no sha256 digests; verify with a .cavssig instead",
            mf_path.display()
        )));
    }

    let mut report = Report {
        files_checked: 0,
        bytes_checked: 0,
        modified: Vec::new(),
        missing: Vec::new(),
        extra: Vec::new(),
    };
    let target_is_dir = target.is_dir();
    for (name, digest) in &digests {
        let path = if target_is_dir {
            target.join(name)
        } else {
            target.to_path_buf()
        };
        if !path.is_file() {
            report.missing.push(name.clone());
            continue;
        }
        report.files_checked += 1;
        report.bytes_checked += std::fs::metadata(&path)?.len();
        if !file_sha256_hex(&path)?.eq_ignore_ascii_case(digest) {
            report.modified.push(name.clone());
        }
    }
    if target_is_dir {
        let known: std::collections::HashSet<&str> =
            digests.iter().map(|(n, _)| n.as_str()).collect();
        for rel in walk_sorted(target)? {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if target.join(&rel).is_file() && !known.contains(rel_str.as_str()) {
                report.extra.push(rel_str);
            }
        }
    }
    Ok(report)
}

fn file_sha256_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
