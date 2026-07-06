//! `cavs diff-plan` — deterministic offline reconstruction plan between an
//! old build (or just its `.cavssig`) and a new build.

use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_plan::{build, BuildOptions, OfflinePlan, PlanKind};
use cavs_proto::errors::ErrorCode;
use cavs_signature::CavsSignature;
use std::path::Path;

pub struct DiffPlanArgs<'a> {
    pub old: Option<&'a Path>,
    pub old_signature: Option<&'a Path>,
    pub new: &'a Path,
    pub out: &'a Path,
    pub analysis: bool,
    pub block_kib: u32,
    pub zstd_level: i32,
    pub report: Option<&'a Path>,
}

pub fn diff_plan(args: &DiffPlanArgs) -> Result<()> {
    let sig = match (args.old_signature, args.old) {
        (Some(sig_path), _) => crate::signature_cmd::load(sig_path)?,
        (None, Some(old)) => {
            let block = args.block_kib.max(1) * 1024;
            let label = old
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if old.is_dir() {
                CavsSignature::sign_dir(old, block, &label)?
            } else {
                CavsSignature::sign_file(old, block, &label)?
            }
        }
        (None, None) => bail!("provide the old build or --old-signature"),
    };

    let opts = BuildOptions {
        kind: if args.analysis {
            PlanKind::Analysis
        } else {
            PlanKind::Portable
        },
        zstd_level: args.zstd_level,
    };
    let plan =
        build(&sig, args.new, &opts).map_err(|e| anyhow::anyhow!(ErrorCode::PlanInvalid.msg(e)))?;
    let encoded = plan.encode(args.zstd_level);
    std::fs::write(args.out, &encoded)
        .with_context(|| format!("cannot write {}", args.out.display()))?;

    let s = plan.summary();
    println!(
        "plan    : {} ({}, {} mode, {})",
        args.out.display(),
        plan.kind.label(),
        plan.mode.label(),
        human_bytes(encoded.len() as u64),
    );
    println!(
        "targets : {} files, {} dirs, {} symlinks, {} deletions ({} total)",
        s.files,
        s.dirs,
        s.symlinks,
        s.deleted,
        human_bytes(plan.new_size),
    );
    println!(
        "sources : {} reused from the old build, {} fresh ({} ops, {} unchanged files)",
        human_bytes(s.reused_bytes),
        human_bytes(s.inline_bytes),
        s.ops_total,
        s.unchanged_files,
    );
    if let Some(report_path) = args.report {
        std::fs::write(report_path, markdown(&plan, encoded.len() as u64))?;
        println!("report  : {}", report_path.display());
    }
    Ok(())
}

fn markdown(plan: &OfflinePlan, encoded_len: u64) -> String {
    let s = plan.summary();
    let mut md = String::new();
    md.push_str("# CAVS reconstruction plan\n\n");
    md.push_str(&format!(
        "`{}` → `{}` ({} mode, {} plan)\n\n",
        plan.old_label,
        plan.new_label,
        plan.mode.label(),
        plan.kind.label()
    ));
    md.push_str("| Metric | Value |\n|---|---:|\n");
    md.push_str(&format!("| Old build | {} |\n", human_bytes(plan.old_size)));
    md.push_str(&format!("| New build | {} |\n", human_bytes(plan.new_size)));
    md.push_str(&format!("| Plan file | {} |\n", human_bytes(encoded_len)));
    md.push_str(&format!(
        "| Reused from old install | {} ({:.1}%) |\n",
        human_bytes(s.reused_bytes),
        s.reused_bytes as f64 * 100.0 / plan.new_size.max(1) as f64
    ));
    md.push_str(&format!(
        "| Fresh data (uncompressed) | {} |\n",
        human_bytes(s.inline_bytes)
    ));
    md.push_str(&format!("| Operations | {} |\n", s.ops_total));
    md.push_str(&format!(
        "| Files new/changed | {} of {} |\n",
        s.files - s.unchanged_files,
        s.files
    ));
    md.push_str(&format!("| Managed deletions | {} |\n", s.deleted));
    md.push_str(
        "\nApply offline with `cavs apply --old <old> --plan <plan> --out <out> --verify`.\n",
    );
    md
}
