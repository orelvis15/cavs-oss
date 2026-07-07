//! `cavs publish-dir` — one command from an exported build folder to a
//! publishable release (v0.8.0).
//!
//! Directory builds are CAVS's first-class shape (the shape engines
//! export). For `./Build_v2` against the previous release this produces,
//! in one deterministic pass:
//!
//! ```text
//! <out>/build_v2.cavs            container (deduplicated data tracks)
//! <out>/build_v2.cavssig         signature for the *next* release to diff
//! <out>/v1_to_v2.cavsplan        offline stream patch vs the previous build
//! <out>/v1_to_v2.cavspatch       optimized sidecar (per-file strategies),
//!                                only when --optimize-patches is not off
//! ```
//!
//! plus a preview: NEW/MODIFIED/DELETED/SAME counts, renames detected
//! (metadata-only, no payload) and compressed-blob warnings before
//! anything ships.

use crate::compare::{classify, detect_renames, FileState};
use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use std::path::Path;

pub struct PublishArgs<'a> {
    pub build: &'a Path,
    /// Previous build directory or its `.cavssig` (optional: first release).
    pub previous: Option<&'a Path>,
    pub out_dir: &'a Path,
    /// off | auto — generate the optimized sidecar for previous→this.
    pub optimize_patches: String,
    pub ignore: Vec<String>,
    pub zstd_level: i32,
    pub sign_key: Option<&'a Path>,
    pub preview_only: bool,
}

pub fn publish_dir(args: &PublishArgs) -> Result<()> {
    if !args.build.is_dir() {
        bail!("{} is not a directory build", args.build.display());
    }
    std::fs::create_dir_all(args.out_dir)?;
    let name = args
        .build
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase().replace(' ', "_"))
        .unwrap_or_else(|| "build".into());

    // ---- Previous signature (from a dir or a .cavssig) --------------------
    let prev_sig = match args.previous {
        None => None,
        Some(p) if p.is_dir() => {
            let label = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            Some((
                cavs_signature::CavsSignature::sign_dir(
                    p,
                    cavs_signature::DEFAULT_BLOCK_SIZE,
                    &label,
                )?,
                Some(p.to_path_buf()),
            ))
        }
        Some(p) => Some((crate::signature_cmd::load(p)?, None)),
    };

    // ---- Preview: what changed, renames, hostile files ---------------------
    if let Some((sig, _)) = &prev_sig {
        let entries = classify(sig, args.build)?;
        let renames = detect_renames(sig, args.build, &entries)?;
        let (mut new, mut modified, mut deleted, mut same) = (0u64, 0u64, 0u64, 0u64);
        for e in &entries {
            match e.state {
                FileState::New => new += 1,
                FileState::Modified => modified += 1,
                FileState::Deleted => deleted += 1,
                FileState::Same => same += 1,
            }
        }
        println!(
            "preview : {new} new · {modified} modified · {deleted} deleted · {same} same \
             · {} renames (metadata-only)",
            renames.len()
        );
        for (to, from) in &renames {
            println!("  renamed: {from} → {to}");
        }
        for e in &entries {
            if e.state != FileState::Modified && e.state != FileState::New {
                continue;
            }
            let full = args.build.join(&e.path);
            if !full.is_file() || e.size < 1024 * 1024 {
                continue;
            }
            let bytes = std::fs::read(&full)?;
            let (shape, magic) = crate::blob_detect::classify_blob(&bytes);
            if shape != crate::blob_detect::BlobShape::Plain {
                println!(
                    "  WARNING: {} looks {} — block-level patching degrades on it; \
                     ship the uncompressed folder instead",
                    e.path,
                    magic.unwrap_or(shape.label()),
                );
            }
        }
    } else {
        println!("preview : first release (no previous version given)");
    }
    if args.preview_only {
        return Ok(());
    }

    // ---- Container ---------------------------------------------------------
    let cavs_out = args.out_dir.join(format!("{name}.cavs"));
    crate::pack_dir::pack_dir(
        args.build,
        &cavs_out,
        &crate::pack_dir::PackDirOptions {
            profile: None,
            compress: true,
            zstd_level: args.zstd_level,
            sign_key: args.sign_key.map(|p| p.to_path_buf()),
            ignore: args.ignore.clone(),
        },
    )?;

    // ---- Signature for the next release ------------------------------------
    let sig_out = args.out_dir.join(format!("{name}.cavssig"));
    crate::signature_cmd::export(
        args.build,
        true,
        cavs_signature::DEFAULT_BLOCK_SIZE,
        &sig_out,
    )?;

    // ---- Patches against the previous release -------------------------------
    if let Some((sig, prev_dir)) = &prev_sig {
        let prev_label = sanitize(&sig.source_label);
        let plan_out = args
            .out_dir
            .join(format!("{prev_label}_to_{name}.cavsplan"));
        let plan = cavs_plan::build(sig, args.build, &cavs_plan::BuildOptions::default())?;
        let encoded = plan.encode(19);
        std::fs::write(&plan_out, &encoded)
            .with_context(|| format!("cannot write {}", plan_out.display()))?;
        println!(
            "plan    : {} ({})",
            plan_out.display(),
            human_bytes(encoded.len() as u64)
        );

        if args.optimize_patches != "off" {
            match prev_dir {
                Some(prev) => {
                    let patch_out = args
                        .out_dir
                        .join(format!("{prev_label}_to_{name}.cavspatch"));
                    let report = crate::patch_v2::generate(
                        prev,
                        args.build,
                        &crate::patch_v2::GenerateOptions::default(),
                        &patch_out,
                    )?;
                    println!(
                        "sidecar : {} ({}, {} copy-old / {} plan-ops / {} bsdiff / {} xdelta3 / {} full)",
                        patch_out.display(),
                        human_bytes(report.patch_bytes),
                        report.files_copy_old,
                        report.files_plan_ops,
                        report.files_bsdiff,
                        report.files_xdelta3,
                        report.files_full_data,
                    );
                }
                None => println!(
                    "sidecar : skipped — optimized patches need the previous build's \
                     bytes (a .cavssig is enough for the plan, not the sidecar)"
                ),
            }
        }
    }
    println!(
        "publish : done — upload {} to your release store",
        args.out_dir.display()
    );
    Ok(())
}

fn sanitize(label: &str) -> String {
    let s: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "previous".into()
    } else {
        s.to_lowercase()
    }
}
