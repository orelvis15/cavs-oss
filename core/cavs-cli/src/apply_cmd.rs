//! `cavs apply` — execute a `.cavsplan` locally, no server involved.
//!
//! Artifact plans write `<out>.part` and rename after verification;
//! directory plans stage into `<out>/.cavs-staging/`, verify, journal and
//! commit per file. Either way a failed apply never leaves corrupt output.

use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_plan::apply::{apply_artifact, apply_dir, ApplyJournal, ApplyOptions, ApplyStats};
use cavs_plan::{OfflinePlan, PlanError, PlanMode};
use cavs_proto::errors::ErrorCode;
use std::path::{Path, PathBuf};

pub struct ApplyArgs<'a> {
    pub old: Option<&'a Path>,
    pub plan: Option<&'a Path>,
    pub out: Option<&'a Path>,
    pub inplace: bool,
    pub verify: bool,
    pub delete_removed: bool,
    pub check_old: bool,
    pub resume: Option<&'a Path>,
    pub json: bool,
}

pub fn apply(args: &ApplyArgs) -> Result<()> {
    // --resume <journal>: re-derive everything the original command knew.
    let (plan_path, old, out): (PathBuf, PathBuf, PathBuf) = if let Some(journal_path) = args.resume
    {
        let bytes = std::fs::read(journal_path)
            .with_context(|| format!("cannot read {}", journal_path.display()))?;
        let journal: ApplyJournal = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!(ErrorCode::JournalCorrupt.msg(e)))?;
        let plan_path = journal.plan_path.clone().ok_or_else(|| {
            anyhow::anyhow!(ErrorCode::JournalResumeFailed
                .msg("journal does not record the plan path; re-run the original apply command"))
        })?;
        (
            plan_path,
            journal.old_root.clone(),
            journal.out_root.clone(),
        )
    } else {
        let plan_path = args.plan.context("--plan is required")?.to_path_buf();
        let old = args.old.context("--old is required")?.to_path_buf();
        let out = match (args.out, args.inplace) {
            (Some(out), false) => out.to_path_buf(),
            (None, true) => old.clone(),
            (Some(_), true) => bail!("--out and --inplace are mutually exclusive"),
            (None, false) => bail!("provide --out, or --inplace to update the old install"),
        };
        (plan_path, old, out)
    };

    let plan = load_plan(&plan_path)?;
    let stats = match plan.mode {
        PlanMode::Artifact => {
            if out.is_dir() {
                bail!(
                    "{} is a directory; artifact plans need a file output",
                    out.display()
                );
            }
            map_err(apply_artifact(&plan, &old, &out))?
        }
        PlanMode::Directory => map_err(apply_dir(
            &plan,
            &old,
            &out,
            &ApplyOptions {
                delete_removed: args.delete_removed,
                check_old: args.check_old,
                plan_path: Some(plan_path.clone()),
            },
        ))?,
    };

    // Belt and braces: --verify re-hashes the final output against the
    // plan (the apply already verified every byte before committing).
    if args.verify {
        for e in &plan.new_entries {
            if e.kind != cavs_signature::EntryKind::File {
                continue;
            }
            let path = match plan.mode {
                PlanMode::Artifact => out.clone(),
                PlanMode::Directory => out.join(&e.path),
            };
            let expected = e.blake3.expect("file entries carry a hash");
            if !cavs_plan::apply::file_matches(&path, e.size, &expected) {
                bail!("{}", ErrorCode::ApplyHashMismatch.msg(e.path.clone()));
            }
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        print_stats(&out, &stats, args.verify);
    }
    Ok(())
}

pub fn load_plan(path: &Path) -> Result<OfflinePlan> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    OfflinePlan::decode(&bytes).map_err(|e| match &e {
        PlanError::Invalid(_) => anyhow::anyhow!(ErrorCode::PlanInvalid.msg(e)),
        PlanError::PathTraversal(_) => anyhow::anyhow!(ErrorCode::PathTraversal.msg(e)),
        _ => anyhow::anyhow!(ErrorCode::PlanCorrupt.msg(e)),
    })
}

fn map_err(r: std::result::Result<ApplyStats, PlanError>) -> Result<ApplyStats> {
    r.map_err(|e| match &e {
        PlanError::ApplyHashMismatch(_) => anyhow::anyhow!(ErrorCode::ApplyHashMismatch.msg(e)),
        PlanError::Journal(_) => anyhow::anyhow!(ErrorCode::JournalResumeFailed.msg(e)),
        PlanError::PathTraversal(_) => anyhow::anyhow!(ErrorCode::PathTraversal.msg(e)),
        PlanError::UnsupportedSymlink(_) => {
            anyhow::anyhow!(ErrorCode::UnsupportedSymlink.msg(e))
        }
        PlanError::NotPortable => anyhow::anyhow!(ErrorCode::PlanInvalid.msg(e)),
        _ => anyhow::anyhow!(ErrorCode::ContainerApplyFailed.msg(e)),
    })
}

fn print_stats(out: &Path, s: &ApplyStats, verified: bool) {
    println!(
        "apply   : OK — {}{}",
        out.display(),
        if verified { " (verified)" } else { "" }
    );
    println!(
        "  files : {} written, {} unchanged (no-op), {} deleted",
        s.files_written, s.files_noop, s.deleted
    );
    println!(
        "  bytes : {} written — {} from the old install, {} from the plan",
        human_bytes(s.bytes_written),
        human_bytes(s.bytes_from_old),
        human_bytes(s.bytes_from_blob),
    );
    println!("  time  : {} ms", s.elapsed_ms);
}
