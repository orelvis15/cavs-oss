//! `cavs verify-install` — does an installed build match a known-good
//! description? Reports the exact mismatch type per entry; exits non-zero
//! on any mismatch (extra files optionally tolerated for mods/saves).

use crate::compare::{classify, FileState};
use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_proto::errors::ErrorCode;
use std::path::Path;

#[derive(serde::Serialize)]
struct VerifyReport {
    target: String,
    against: String,
    ok: bool,
    files_checked: u64,
    bytes_checked: u64,
    modified: Vec<String>,
    missing: Vec<String>,
    extra: Vec<String>,
    elapsed_ms: u64,
}

pub fn verify_install(
    target: &Path,
    signature: Option<&Path>,
    manifest: Option<&Path>,
    allow_extra: bool,
    json: bool,
) -> Result<()> {
    let started = std::time::Instant::now();
    let mut report = match (signature, manifest) {
        (Some(sig_path), None) => verify_against_signature(target, sig_path)?,
        (None, Some(mf_path)) => verify_against_manifest(target, mf_path)?,
        _ => bail!("provide exactly one of --signature or --manifest"),
    };
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    if allow_extra {
        report.extra.clear();
    }
    report.ok = report.modified.is_empty() && report.missing.is_empty() && report.extra.is_empty();

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.ok {
        println!("OK {} matches {}", report.target, report.against);
        println!("  files checked: {}", report.files_checked);
        println!("  bytes checked: {}", human_bytes(report.bytes_checked));
        println!("  elapsed: {} ms", report.elapsed_ms);
    } else {
        println!("verification failed");
        for p in &report.modified {
            println!("  MODIFIED {p}");
        }
        for p in &report.missing {
            println!("  MISSING {p}");
        }
        for p in &report.extra {
            println!("  EXTRA {p}");
        }
    }
    if !report.ok {
        bail!(
            "{}",
            ErrorCode::SignatureMismatch.msg(format!(
                "{} modified, {} missing, {} extra",
                report.modified.len(),
                report.missing.len(),
                report.extra.len()
            ))
        );
    }
    Ok(())
}

fn verify_against_signature(target: &Path, sig_path: &Path) -> Result<VerifyReport> {
    let sig = crate::signature_cmd::load(sig_path)?;
    let entries = classify(&sig, target)?;
    let mut report = VerifyReport {
        target: target.display().to_string(),
        against: sig_path.display().to_string(),
        ok: false,
        files_checked: 0,
        bytes_checked: 0,
        modified: Vec::new(),
        missing: Vec::new(),
        extra: Vec::new(),
        elapsed_ms: 0,
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

/// Verify against a manifest's recorded SHA-256 digests (`sha256:<name>`
/// meta entries, one per data track — how the packer describes outputs).
fn verify_against_manifest(target: &Path, mf_path: &Path) -> Result<VerifyReport> {
    let bytes =
        std::fs::read(mf_path).with_context(|| format!("cannot read {}", mf_path.display()))?;
    let loaded = cavs_manifest::read_manifest(&bytes)
        .map_err(|e| anyhow::anyhow!(ErrorCode::ManifestCorrupt.msg(e)))?;
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
        bail!(
            "{} records no sha256 digests; verify with a .cavssig instead",
            mf_path.display()
        );
    }

    let mut report = VerifyReport {
        target: target.display().to_string(),
        against: mf_path.display().to_string(),
        ok: false,
        files_checked: 0,
        bytes_checked: 0,
        modified: Vec::new(),
        missing: Vec::new(),
        extra: Vec::new(),
        elapsed_ms: 0,
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
        if file_sha256_hex(&path)?.eq_ignore_ascii_case(digest) {
            // matches
        } else {
            report.modified.push(name.clone());
        }
    }
    if target_is_dir {
        let known: std::collections::HashSet<&str> =
            digests.iter().map(|(n, _)| n.as_str()).collect();
        for rel in crate::compare::walk_sorted(target)? {
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
