//! `cavs test apply-recovery` — prove that an interrupted or attacked
//! apply never leaves a broken install (v0.8.0).
//!
//! The harness spawns *real* `cavs apply` subprocesses against a fresh
//! copy of the old install and SIGKILLs them at ramping delays until one
//! run completes untouched. After every kill it asserts the safety
//! invariant — each managed file is byte-for-byte the old version or the
//! new version, never a torn mix; user files are untouched — and then
//! proves the journaled resume finishes the update. Corruption cases
//! (flipped plan bytes, wrong old install, garbage in the staging area)
//! must fail cleanly or self-heal, never commit bad bytes.

use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_hash::hash_chunk;
use std::collections::HashMap;
use std::path::Path;

#[derive(serde::Serialize)]
pub struct CaseResult {
    pub case: String,
    pub runs: u64,
    pub ok: bool,
    pub detail: String,
}

#[derive(Default, serde::Serialize)]
pub struct RecoveryReport {
    pub old: String,
    pub new: String,
    pub plan_bytes: u64,
    pub cases: Vec<CaseResult>,
    pub all_ok: bool,
}

pub fn run(old: &Path, new: &Path, out: Option<&Path>) -> Result<()> {
    if !old.is_dir() || !new.is_dir() {
        bail!("--old and --new must be directory builds");
    }
    let work = tempfile::tempdir()?;
    let label = old
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let sig =
        cavs_signature::CavsSignature::sign_dir(old, cavs_signature::DEFAULT_BLOCK_SIZE, &label)?;
    let plan = cavs_plan::build(&sig, new, &cavs_plan::BuildOptions::default())?;
    let plan_path = work.path().join("update.cavsplan");
    let plan_bytes = plan.encode(19);
    std::fs::write(&plan_path, &plan_bytes)?;

    let old_files = tree_contents(old)?;
    let new_files = tree_contents(new)?;

    let mut report = RecoveryReport {
        old: old.display().to_string(),
        new: new.display().to_string(),
        plan_bytes: plan_bytes.len() as u64,
        ..Default::default()
    };

    // ---- Case 1: SIGKILL at ramping delays + journaled resume -------------
    report.cases.push(kill_and_resume_case(
        old,
        &plan_path,
        &old_files,
        &new_files,
        work.path(),
    )?);

    // ---- Case 2: corrupt plan is rejected, nothing written ----------------
    {
        let mut bad = plan_bytes.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 0xff;
        let bad_path = work.path().join("corrupt.cavsplan");
        std::fs::write(&bad_path, &bad)?;
        let install = work.path().join("install-corrupt-plan");
        crate::bench_butler::copy_tree(old, &install)?;
        let status = apply_cmd(&install, &bad_path).status()?;
        let untouched = tree_contents(&install)? == old_files;
        report.cases.push(CaseResult {
            case: "corrupt plan".into(),
            runs: 1,
            ok: !status.success() && untouched,
            detail: if status.success() {
                "apply accepted a corrupt plan".into()
            } else if !untouched {
                "install modified by a rejected plan".into()
            } else {
                "rejected cleanly, install untouched".into()
            },
        });
    }

    // ---- Case 3: corrupt old install → never commit bad bytes -------------
    // Two outcomes are correct: the apply fails cleanly (hash mismatch,
    // corrupt file untouched), or it self-heals — deduplicated content can
    // source the damaged range from another file, in which case the final
    // tree must be byte-for-byte the new build. What must never happen is
    // committing bytes that are neither.
    {
        let install = work.path().join("install-wrong-old");
        crate::bench_butler::copy_tree(old, &install)?;
        if let Some((rel, bytes)) = old_files
            .iter()
            .find(|(r, b)| !b.is_empty() && new_files.get(*r).map(|n| n != *b).unwrap_or(false))
        {
            let mut corrupted = bytes.clone();
            for b in corrupted.iter_mut().take(4096) {
                *b ^= 0xa5;
            }
            std::fs::write(install.join(rel), &corrupted)?;
            let status = apply_cmd(&install, &plan_path).status()?;
            let (ok, detail) = if status.success() {
                if managed_tree_matches(&install, &new_files)? {
                    (
                        true,
                        "self-healed: damaged range sourced from deduplicated \
                         content elsewhere; output verified byte-identical"
                            .to_string(),
                    )
                } else {
                    (
                        false,
                        "apply reported success but the tree does not match the new build"
                            .to_string(),
                    )
                }
            } else {
                let got = std::fs::read(install.join(rel))?;
                let untouched_or_new =
                    got == corrupted || new_files.get(rel).map(|n| *n == got).unwrap_or(false);
                if untouched_or_new {
                    (true, "failed cleanly, nothing bad committed".to_string())
                } else {
                    (false, format!("{rel} left in a torn state"))
                }
            };
            report.cases.push(CaseResult {
                case: "corrupt old install".into(),
                runs: 1,
                ok,
                detail,
            });
        }
    }

    // ---- Case 4: garbage staged files self-heal on the next run -----------
    {
        let install = work.path().join("install-garbage-staging");
        crate::bench_butler::copy_tree(old, &install)?;
        let staging = install.join(".cavs-staging");
        std::fs::create_dir_all(&staging)?;
        for i in 0..8 {
            std::fs::write(staging.join(format!("e{i}")), b"partial garbage")?;
        }
        let status = apply_cmd(&install, &plan_path).status()?;
        let matches = managed_tree_matches(&install, &new_files)?;
        report.cases.push(CaseResult {
            case: "garbage in staging".into(),
            runs: 1,
            ok: status.success() && matches,
            detail: if status.success() && matches {
                "re-staged and committed correctly".into()
            } else {
                "apply did not recover from garbage staging".into()
            },
        });
    }

    report.all_ok = report.cases.iter().all(|c| c.ok);
    print_report(&report);
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("apply-recovery.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("results : {}/apply-recovery.json", dir.display());
    }
    if !report.all_ok {
        bail!(
            "apply-recovery: {} case(s) failed",
            report.cases.iter().filter(|c| !c.ok).count()
        );
    }
    Ok(())
}

/// Kill `cavs apply` at ramping delays; after each kill assert no torn
/// files, then resume and assert the final tree.
fn kill_and_resume_case(
    old: &Path,
    plan_path: &Path,
    old_files: &HashMap<String, Vec<u8>>,
    new_files: &HashMap<String, Vec<u8>>,
    work: &Path,
) -> Result<CaseResult> {
    let mut runs = 0u64;
    let mut delay_ms = 2u64;
    loop {
        runs += 1;
        let install = work.join(format!("install-kill-{runs}"));
        crate::bench_butler::copy_tree(old, &install)?;
        std::fs::create_dir_all(install.join("mods"))?;
        std::fs::write(install.join("mods/user_mod.pck"), b"my mod")?;

        let mut child = apply_cmd(&install, plan_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("spawning cavs apply")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(delay_ms);
        let completed = loop {
            if let Some(_status) = child.try_wait()? {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().ok();
                child.wait().ok();
                break false;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        };

        // Invariant after a kill: every managed file is exactly the old or
        // the new version — never a torn mix; the mod file survives.
        for (rel, bytes) in tree_contents(&install)? {
            if rel.starts_with(".cavs-staging") || rel == ".cavs-journal.json" {
                continue;
            }
            if rel == "mods/user_mod.pck" {
                if bytes != b"my mod" {
                    return Ok(CaseResult {
                        case: "kill + resume".into(),
                        runs,
                        ok: false,
                        detail: format!("mod file damaged after kill at {delay_ms} ms"),
                    });
                }
                continue;
            }
            let h = hash_chunk(&bytes);
            let is_old = old_files
                .get(&rel)
                .map(|b| hash_chunk(b) == h)
                .unwrap_or(false);
            let is_new = new_files
                .get(&rel)
                .map(|b| hash_chunk(b) == h)
                .unwrap_or(false);
            if !is_old && !is_new {
                return Ok(CaseResult {
                    case: "kill + resume".into(),
                    runs,
                    ok: false,
                    detail: format!("torn file {rel} after kill at {delay_ms} ms"),
                });
            }
        }

        // Resume: re-running the apply must finish the update.
        let status = apply_cmd(&install, plan_path).status()?;
        if !status.success() || !managed_tree_matches(&install, new_files)? {
            return Ok(CaseResult {
                case: "kill + resume".into(),
                runs,
                ok: false,
                detail: format!("resume failed after kill at {delay_ms} ms"),
            });
        }
        let _ = std::fs::remove_dir_all(&install);

        if completed {
            return Ok(CaseResult {
                case: "kill + resume".into(),
                runs,
                ok: true,
                detail: format!(
                    "{} interrupted runs recovered (kills from 2 ms up to {delay_ms} ms, \
                     then one uninterrupted run)",
                    runs - 1
                ),
            });
        }
        delay_ms = (delay_ms as f64 * 1.6).ceil() as u64;
        if runs > 64 {
            return Ok(CaseResult {
                case: "kill + resume".into(),
                runs,
                ok: false,
                detail: "apply never completed within the ramp".into(),
            });
        }
    }
}

fn apply_cmd(install: &Path, plan: &Path) -> std::process::Command {
    let exe = std::env::current_exe().expect("current exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("apply")
        .arg("--old")
        .arg(install)
        .arg("--plan")
        .arg(plan)
        .arg("--inplace")
        .arg("--delete-removed-files");
    cmd
}

fn tree_contents(root: &Path) -> Result<HashMap<String, Vec<u8>>> {
    let mut out = HashMap::new();
    for rel in crate::compare::walk_sorted(root)? {
        let full = root.join(&rel);
        if full.is_file() {
            out.insert(
                rel.to_string_lossy().replace('\\', "/"),
                std::fs::read(&full)?,
            );
        }
    }
    Ok(out)
}

/// Every managed (plan-covered) file equals the new build; extra files are
/// ignored (mods).
fn managed_tree_matches(install: &Path, new_files: &HashMap<String, Vec<u8>>) -> Result<bool> {
    let got = tree_contents(install)?;
    for (rel, bytes) in new_files {
        match got.get(rel) {
            Some(b) if b == bytes => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn print_report(r: &RecoveryReport) {
    println!(
        "test apply-recovery: {} → {} (plan {})",
        r.old,
        r.new,
        human_bytes(r.plan_bytes)
    );
    for c in &r.cases {
        println!(
            "  {:<24} {}  {}",
            c.case,
            if c.ok { "OK " } else { "FAIL" },
            c.detail
        );
    }
}
