//! `cavs certify integrity` — every output and intermediate file must be
//! valid, safe and byte-identical, and corrupt inputs must fail cleanly.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cavs_plan::apply::{apply_artifact, apply_dir, ApplyOptions, ApplyStats};
use cavs_plan::{build, BuildOptions, OfflinePlan, PlanKind, PlanMode};
use cavs_signature::CavsSignature;

use super::{worst, CheckResult, CheckRow};
use crate::report::human_bytes;

const BLOCK_SIZE: u32 = 64 * 1024;
const PLAN_ZSTD: i32 = 19;

pub struct Inputs<'a> {
    pub old: Option<&'a Path>,
    pub new: Option<&'a Path>,
    pub signature_old: Option<&'a Path>,
    pub signature_new: Option<&'a Path>,
    pub plan: Option<&'a Path>,
    /// Corrupt-signature / corrupt-plan / corrupt-old smoke checks.
    pub corruption_checks: bool,
    /// Re-plan new→new and assert a 0-byte payload.
    pub noop_check: bool,
}

pub struct Outcome {
    pub rows: Vec<CheckRow>,
    pub result: CheckResult,
    pub byte_identical: bool,
    pub plan_path: Option<PathBuf>,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(serde::Serialize)]
struct Hashes {
    old_blake3: Option<String>,
    new_blake3: Option<String>,
    output_verified_against: String,
    old_merkle_root: String,
    new_merkle_root: String,
    plan_file_blake3: String,
}

fn sign(path: &Path, label: &str) -> Result<CavsSignature> {
    if path.is_dir() {
        CavsSignature::sign_dir(path, BLOCK_SIZE, label)
            .map_err(|e| anyhow::anyhow!("signing {} failed: {e}", path.display()))
    } else {
        CavsSignature::sign_file(path, BLOCK_SIZE, label)
            .map_err(|e| anyhow::anyhow!("signing {} failed: {e}", path.display()))
    }
}

fn hash_hex(h: &Option<cavs_hash::ChunkHash>) -> Option<String> {
    h.as_ref().map(cavs_hash::to_hex)
}

/// Flip one byte in the middle of a byte buffer.
fn flip_middle(bytes: &mut [u8]) {
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
}

/// Corrupt a copy of an old build at a byte the plan actually reads (the
/// middle of its largest copy-old range) and return the copy's path.
/// Returns None when the plan reuses no old bytes at all.
fn corrupt_copy(old: &Path, plan: &OfflinePlan, scratch: &Path) -> Result<Option<PathBuf>> {
    use cavs_plan::PlanOp;
    let Some((entry_id, offset, len)) = plan
        .ops
        .iter()
        .filter_map(|op| match op {
            PlanOp::CopyOld {
                old_entry_id,
                old_offset,
                len,
                ..
            } if *len > 0 => Some((*old_entry_id, *old_offset, *len)),
            _ => None,
        })
        .max_by_key(|(_, _, len)| *len)
    else {
        return Ok(None);
    };
    let dst = scratch.join("corrupt-old");
    let victim = if old.is_dir() {
        copy_tree(old, &dst)?;
        let rel = plan
            .old_entries
            .iter()
            .find(|e| e.entry_id == entry_id)
            .context("corrupt-old: copy op references an unknown old entry")?
            .path
            .clone();
        dst.join(rel)
    } else {
        std::fs::copy(old, &dst)?;
        dst.clone()
    };
    let mut bytes = std::fs::read(&victim)?;
    let at = ((offset + len / 2) as usize).min(bytes.len().saturating_sub(1));
    bytes[at] ^= 0xFF;
    std::fs::write(&victim, bytes)?;
    Ok(Some(dst))
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn unsafe_path(p: &str) -> bool {
    let path = Path::new(p);
    path.is_absolute() || p.contains("..") || p.contains(":\\") || p.starts_with('\\')
}

fn apply_plan(plan: &OfflinePlan, old: &Path, out: &Path) -> Result<ApplyStats> {
    match plan.mode {
        PlanMode::Artifact => {
            apply_artifact(plan, old, out).map_err(|e| anyhow::anyhow!("apply failed: {e}"))
        }
        PlanMode::Directory => apply_dir(
            plan,
            old,
            out,
            &ApplyOptions {
                delete_removed: true,
                check_old: false,
                plan_path: None,
            },
        )
        .map_err(|e| anyhow::anyhow!("apply failed: {e}")),
    }
}

pub fn run(inputs: &Inputs, out_dir: &Path, commands: &mut Vec<String>) -> Result<Outcome> {
    let artifacts = out_dir.join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    let scratch = tempfile::tempdir().context("cannot create scratch directory")?;

    let mut rows: Vec<CheckRow> = Vec::new();
    let mut metrics: BTreeMap<String, f64> = BTreeMap::new();
    let mut byte_identical = false;

    let (old, new) = match (inputs.old, inputs.new) {
        (Some(o), Some(n)) => (o, n),
        _ => {
            return run_existing_artifacts(inputs, out_dir, &mut rows);
        }
    };
    if old.is_dir() != new.is_dir() {
        bail!("CAVS-E-CERTIFY-INPUT: old and new must both be files or both be directories");
    }

    // -- Signatures ---------------------------------------------------------
    let old_sig = sign(old, &old.display().to_string())?;
    let old_sig_bytes = old_sig.encode();
    let old_sig_path = artifacts.join("old.cavssig");
    std::fs::write(&old_sig_path, &old_sig_bytes)?;
    commands.push(format!(
        "cavs signature export --raw {} -o artifacts/old.cavssig",
        old.display()
    ));
    let old_sig = CavsSignature::decode(&old_sig_bytes)
        .map_err(|e| anyhow::anyhow!("old signature decode failed: {e}"))?;
    rows.push(CheckRow::new(
        "old signature export+decode",
        CheckResult::Pass,
        format!(
            "{} entries, {}",
            old_sig.entries.len(),
            human_bytes(old_sig_bytes.len() as u64)
        ),
    ));
    metrics.insert("signature_old_bytes".into(), old_sig_bytes.len() as f64);

    let new_sig = sign(new, &new.display().to_string())?;
    let new_sig_bytes = new_sig.encode();
    let new_sig_path = artifacts.join("new.cavssig");
    std::fs::write(&new_sig_path, &new_sig_bytes)?;
    commands.push(format!(
        "cavs signature export --raw {} -o artifacts/new.cavssig",
        new.display()
    ));
    let new_sig = CavsSignature::decode(&new_sig_bytes)
        .map_err(|e| anyhow::anyhow!("new signature decode failed: {e}"))?;
    rows.push(CheckRow::new(
        "new signature export+decode",
        CheckResult::Pass,
        format!(
            "{} entries, {}",
            new_sig.entries.len(),
            human_bytes(new_sig_bytes.len() as u64)
        ),
    ));
    metrics.insert("signature_new_bytes".into(), new_sig_bytes.len() as f64);

    // Signatures must describe their sources exactly.
    match old_sig.verify_against(old) {
        Ok(()) => rows.push(CheckRow::new(
            "old signature verify",
            CheckResult::Pass,
            "every block hash matches the source",
        )),
        Err(e) => rows.push(CheckRow::new(
            "old signature verify",
            CheckResult::Fail,
            format!("{e}"),
        )),
    }

    // -- Plan build + decode roundtrip --------------------------------------
    let plan = build(
        &old_sig,
        new,
        &BuildOptions {
            kind: PlanKind::Portable,
            zstd_level: PLAN_ZSTD,
        },
    )
    .map_err(|e| anyhow::anyhow!("plan build failed: {e}"))?;
    let plan_bytes = plan.encode(PLAN_ZSTD);
    let plan_path = artifacts.join("update.cavsplan");
    std::fs::write(&plan_path, &plan_bytes)?;
    commands.push(format!(
        "cavs diff-plan {} {} --out artifacts/update.cavsplan",
        old.display(),
        new.display()
    ));
    let plan =
        OfflinePlan::decode(&plan_bytes).map_err(|e| anyhow::anyhow!("plan decode failed: {e}"))?;
    let summary = plan.summary();
    rows.push(CheckRow::new(
        "plan build+decode",
        CheckResult::Pass,
        format!(
            "CAVSPLAN1, {} ops, payload {} ({} on the wire)",
            summary.ops_total,
            human_bytes(summary.inline_bytes),
            human_bytes(plan_bytes.len() as u64)
        ),
    ));
    metrics.insert("plan_bytes".into(), plan_bytes.len() as f64);
    metrics.insert("network_bytes".into(), plan_bytes.len() as f64);
    metrics.insert("plan_ops_total".into(), summary.ops_total as f64);
    metrics.insert("plan_inline_bytes".into(), summary.inline_bytes as f64);

    // -- Path safety ---------------------------------------------------------
    let mut unsafe_paths: Vec<&str> = plan
        .new_entries
        .iter()
        .map(|e| e.path.as_str())
        .filter(|p| unsafe_path(p))
        .collect();
    unsafe_paths.extend(
        plan.deleted
            .iter()
            .map(String::as_str)
            .filter(|p| unsafe_path(p)),
    );
    rows.push(if unsafe_paths.is_empty() {
        CheckRow::new(
            "no path traversal",
            CheckResult::Pass,
            format!(
                "0 unsafe paths in {} entries + {} deletions",
                plan.new_entries.len(),
                plan.deleted.len()
            ),
        )
    } else {
        CheckRow::new(
            "no path traversal",
            CheckResult::Fail,
            format!("unsafe paths: {}", unsafe_paths.join(", ")),
        )
    });

    // -- Apply + byte-identical ----------------------------------------------
    let apply_out = if new.is_dir() {
        scratch.path().join("apply-out")
    } else {
        scratch.path().join("apply-out.bin")
    };
    commands.push(format!(
        "cavs apply --old {} --plan artifacts/update.cavsplan --out <tmp> --verify",
        old.display()
    ));
    match apply_plan(&plan, old, &apply_out) {
        Ok(stats) => {
            metrics.insert("apply_ms".into(), stats.elapsed_ms as f64);
            match new_sig.verify_against(&apply_out) {
                Ok(()) => {
                    byte_identical = true;
                    rows.push(CheckRow::new(
                        "apply output byte-identical",
                        CheckResult::Pass,
                        format!(
                            "verified against new signature in {} ms ({} from old, {} fresh)",
                            stats.elapsed_ms,
                            human_bytes(stats.bytes_from_old),
                            human_bytes(stats.bytes_from_blob)
                        ),
                    ));
                }
                Err(e) => rows.push(CheckRow::new(
                    "apply output byte-identical",
                    CheckResult::Fail,
                    format!("output does not match the new build: {e}"),
                )),
            }
        }
        Err(e) => rows.push(CheckRow::new(
            "apply output byte-identical",
            CheckResult::Fail,
            format!("{e:#}"),
        )),
    }

    // -- No-op reapply ----------------------------------------------------------
    // Re-updating a client that already has the new version must succeed
    // and rewrite nothing: a directory plan applied in place detects every
    // file as a no-op; an artifact plan reconstructs from local bytes only
    // (its payload carries at most the sub-block tail of the file).
    if inputs.noop_check {
        let noop_plan = build(
            &new_sig,
            new,
            &BuildOptions {
                kind: PlanKind::Portable,
                zstd_level: PLAN_ZSTD,
            },
        )
        .map_err(|e| anyhow::anyhow!("no-op plan build failed: {e}"))?;
        let row = if new.is_dir() {
            let inplace = scratch.path().join("noop-inplace");
            copy_tree(new, &inplace)?;
            match apply_plan(&noop_plan, &inplace, &inplace).and_then(|stats| {
                new_sig
                    .verify_against(&inplace)
                    .map_err(|e| anyhow::anyhow!("no-op output mismatch: {e}"))
                    .map(|()| stats)
            }) {
                Ok(stats) if stats.files_written == 0 => CheckRow::new(
                    "no-op reapply",
                    CheckResult::Pass,
                    format!(
                        "0 files rewritten ({} detected as no-op); output byte-identical",
                        stats.files_noop
                    ),
                ),
                Ok(stats) => CheckRow::new(
                    "no-op reapply",
                    CheckResult::Warn,
                    format!(
                        "output byte-identical but {} files were rewritten",
                        stats.files_written
                    ),
                ),
                Err(e) => CheckRow::new("no-op reapply", CheckResult::Fail, format!("{e:#}")),
            }
        } else {
            let noop_out = scratch.path().join("noop-out.bin");
            match apply_plan(&noop_plan, new, &noop_out).and_then(|stats| {
                new_sig
                    .verify_against(&noop_out)
                    .map_err(|e| anyhow::anyhow!("no-op output mismatch: {e}"))
                    .map(|()| stats)
            }) {
                Ok(stats) if stats.bytes_from_blob <= BLOCK_SIZE as u64 => CheckRow::new(
                    "no-op reapply",
                    CheckResult::Pass,
                    format!(
                        "reconstructed from local bytes ({} payload — the sub-block tail); \
                         output byte-identical",
                        human_bytes(stats.bytes_from_blob)
                    ),
                ),
                Ok(stats) => CheckRow::new(
                    "no-op reapply",
                    CheckResult::Warn,
                    format!(
                        "output byte-identical but payload was {}",
                        human_bytes(stats.bytes_from_blob)
                    ),
                ),
                Err(e) => CheckRow::new("no-op reapply", CheckResult::Fail, format!("{e:#}")),
            }
        };
        rows.push(row);
    } else {
        rows.push(CheckRow::new(
            "no-op reapply",
            CheckResult::Skipped,
            "not part of the quick profile",
        ));
    }

    // -- Corruption smoke ---------------------------------------------------------
    if inputs.corruption_checks {
        // Corrupt signature must be rejected.
        let mut bad_sig = new_sig_bytes.clone();
        flip_middle(&mut bad_sig);
        rows.push(match CavsSignature::decode(&bad_sig) {
            Err(_) => CheckRow::new(
                "corrupt signature rejected",
                CheckResult::Pass,
                "decoder refused a bit-flipped .cavssig",
            ),
            Ok(_) => CheckRow::new(
                "corrupt signature rejected",
                CheckResult::Fail,
                "decoder accepted a bit-flipped .cavssig",
            ),
        });

        // Corrupt plan must be rejected.
        let mut bad_plan = plan_bytes.clone();
        flip_middle(&mut bad_plan);
        rows.push(match OfflinePlan::decode(&bad_plan) {
            Err(_) => CheckRow::new(
                "corrupt plan rejected",
                CheckResult::Pass,
                "decoder refused a bit-flipped .cavsplan",
            ),
            Ok(_) => CheckRow::new(
                "corrupt plan rejected",
                CheckResult::Fail,
                "decoder accepted a bit-flipped .cavsplan",
            ),
        });

        // Corrupt old input must fail the apply, and must not leave output.
        match corrupt_copy(old, &plan, scratch.path())? {
            None => rows.push(CheckRow::new(
                "corrupted old input fails safely",
                CheckResult::Skipped,
                "the plan reuses no old bytes (fully new build)",
            )),
            Some(corrupt_old) => {
                let corrupt_out = if new.is_dir() {
                    scratch.path().join("corrupt-apply-out")
                } else {
                    scratch.path().join("corrupt-apply-out.bin")
                };
                let corrupt_result = apply_plan(&plan, &corrupt_old, &corrupt_out).and_then(|_| {
                    new_sig
                        .verify_against(&corrupt_out)
                        .map_err(|e| anyhow::anyhow!("{e}"))
                });
                rows.push(match corrupt_result {
                    Err(_) => CheckRow::new(
                        "corrupted old input fails safely",
                        CheckResult::Pass,
                        "a bit flipped inside a reused old range never produced a \
                         verified output",
                    ),
                    Ok(()) => CheckRow::new(
                        "corrupted old input fails safely",
                        CheckResult::Fail,
                        "apply read a corrupted old range and still verified — \
                         old bytes are not being hash-checked",
                    ),
                });
            }
        }
    } else {
        rows.push(CheckRow::new(
            "corruption smoke checks",
            CheckResult::Skipped,
            "strict profile only (or `cavs certify integrity`)",
        ));
    }

    // -- Hashes artifact ------------------------------------------------------------
    let hashes = Hashes {
        old_blake3: hash_hex(&old_sig.source_blake3),
        new_blake3: hash_hex(&new_sig.source_blake3),
        output_verified_against: "new.cavssig".into(),
        old_merkle_root: cavs_hash::to_hex(&old_sig.merkle_root),
        new_merkle_root: cavs_hash::to_hex(&new_sig.merkle_root),
        plan_file_blake3: cavs_hash::to_hex(&blake3_of(&plan_bytes)),
    };
    std::fs::write(
        artifacts.join("hashes.json"),
        serde_json::to_vec_pretty(&hashes)?,
    )?;

    metrics.insert(
        "byte_identical".into(),
        if byte_identical { 1.0 } else { 0.0 },
    );
    let result = worst(&rows);
    Ok(Outcome {
        rows,
        result,
        byte_identical,
        plan_path: Some(plan_path),
        metrics,
    })
}

fn blake3_of(bytes: &[u8]) -> cavs_hash::ChunkHash {
    cavs_hash::hash_chunk(bytes)
}

/// Existing-artifacts mode: only decode/validate what was passed in.
fn run_existing_artifacts(
    inputs: &Inputs,
    _out_dir: &Path,
    rows: &mut Vec<CheckRow>,
) -> Result<Outcome> {
    let mut check_sig = |name: &str, path: Option<&Path>| -> Result<()> {
        if let Some(p) = path {
            let bytes = std::fs::read(p).with_context(|| format!("cannot read {}", p.display()))?;
            rows.push(match CavsSignature::decode(&bytes) {
                Ok(sig) => CheckRow::new(
                    name,
                    CheckResult::Pass,
                    format!("{} entries", sig.entries.len()),
                ),
                Err(e) => CheckRow::new(name, CheckResult::Fail, format!("{e}")),
            });
        }
        Ok(())
    };
    check_sig("old signature decode", inputs.signature_old)?;
    check_sig("new signature decode", inputs.signature_new)?;

    let mut plan_path = None;
    if let Some(p) = inputs.plan {
        let bytes = std::fs::read(p).with_context(|| format!("cannot read {}", p.display()))?;
        match OfflinePlan::decode(&bytes) {
            Ok(plan) => {
                let s = plan.summary();
                let unsafe_count = plan
                    .new_entries
                    .iter()
                    .map(|e| e.path.as_str())
                    .chain(plan.deleted.iter().map(String::as_str))
                    .filter(|p| unsafe_path(p))
                    .count();
                rows.push(CheckRow::new(
                    "plan decode",
                    CheckResult::Pass,
                    format!("CAVSPLAN1, {} ops", s.ops_total),
                ));
                rows.push(if unsafe_count == 0 {
                    CheckRow::new("no path traversal", CheckResult::Pass, "0 unsafe paths")
                } else {
                    CheckRow::new(
                        "no path traversal",
                        CheckResult::Fail,
                        format!("{unsafe_count} unsafe paths"),
                    )
                });
                plan_path = Some(p.to_path_buf());
            }
            Err(e) => rows.push(CheckRow::new(
                "plan decode",
                CheckResult::Fail,
                format!("{e}"),
            )),
        }
    }
    rows.push(CheckRow::new(
        "apply output byte-identical",
        CheckResult::Skipped,
        "needs --old and --new build bytes",
    ));
    let result = worst(rows);
    Ok(Outcome {
        rows: rows.clone(),
        result,
        byte_identical: false,
        plan_path,
        metrics: BTreeMap::new(),
    })
}

#[derive(serde::Serialize)]
struct Report<'a> {
    schema: &'static str,
    result: CheckResult,
    byte_identical: bool,
    checks: &'a [CheckRow],
    metrics: &'a BTreeMap<String, f64>,
}

pub fn write_reports(outcome: &Outcome, out_dir: &Path) -> Result<()> {
    let report = Report {
        schema: "cavs-certify-integrity/1",
        result: outcome.result,
        byte_identical: outcome.byte_identical,
        checks: &outcome.rows,
        metrics: &outcome.metrics,
    };
    std::fs::write(
        out_dir.join("integrity.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let mut md = String::from("# Integrity Certification\n\n");
    md.push_str(&format!("Result: **{}**\n\n", outcome.result.label()));
    md.push_str(&super::rows_markdown(&outcome.rows));
    md.push_str(
        "\nByte-identical reconstruction is mandatory: any hash mismatch fails \
         the certification.\n",
    );
    std::fs::write(out_dir.join("integrity.md"), md)?;
    Ok(())
}
