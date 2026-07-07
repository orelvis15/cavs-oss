//! `cavs bench steampipe-style` and `cavs analyze steampipe` (v0.9.0).
//!
//! `bench steampipe-style` runs the public fixed-1MiB chunk model and
//! reports the numbers. `analyze steampipe` runs the model plus the
//! detectors and explains *why* an update is expensive and how to fix it.
//! Both label their output as a SteamPipe-style estimate, never as
//! Valve's implementation.

use crate::ignore::IgnoreRules;
use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_analyzer::compare::{analyze, Analysis};
use cavs_analyzer::detect::Thresholds;
use cavs_analyzer::steampipe::{estimate, Compression, Estimate, ModelConfig, Scope};
use cavs_analyzer::Engine;
use std::path::Path;

pub struct BenchArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub chunk_size: Option<&'a str>,
    pub compression: &'a str,
    pub scope: &'a str,
    pub ignore: Vec<String>,
    pub json: bool,
    pub markdown: Option<&'a Path>,
    pub out: Option<&'a Path>,
}

fn keeper(root: &Path, patterns: &[String]) -> Result<impl Fn(&str) -> bool> {
    let rules = IgnoreRules::load(
        if root.is_dir() {
            root
        } else {
            root.parent().unwrap_or(root)
        },
        patterns,
    )?;
    Ok(move |rel: &str| !rules.matches(rel, false))
}

fn check_pair(old: &Path, new: &Path) -> Result<()> {
    for p in [old, new] {
        if !p.exists() {
            bail!(
                "CAVS-E-ANALYZE-PATH-MISSING: {} does not exist",
                p.display()
            );
        }
    }
    if old.is_dir() != new.is_dir() {
        bail!("old and new must both be files or both be directories");
    }
    Ok(())
}

pub fn bench(args: &BenchArgs) -> Result<()> {
    check_pair(args.old, args.new)?;
    let chunk_size = match args.chunk_size {
        Some(s) => crate::synth::parse_size_pub(s)? as usize,
        None => cavs_analyzer::steampipe::DEFAULT_CHUNK,
    };
    let compression = Compression::parse(args.compression).ok_or_else(|| {
        anyhow::anyhow!(
            "CAVS-E-STEAMPIPE-MODEL-INVALID: unknown compression '{}' (none|zstd-N)",
            args.compression
        )
    })?;
    let scope = Scope::parse(args.scope).ok_or_else(|| {
        anyhow::anyhow!(
            "CAVS-E-STEAMPIPE-MODEL-INVALID: unknown scope '{}' (per-file|global)",
            args.scope
        )
    })?;
    let cfg = ModelConfig {
        chunk_size,
        compression,
        scope,
    };
    let keep = keeper(args.new, &args.ignore)?;
    let est = estimate(args.old, args.new, &cfg, &keep)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&est)?);
    } else {
        print_estimate(&est);
    }
    let md = estimate_markdown(&est);
    if let Some(path) = args.markdown {
        std::fs::write(path, &md)?;
        eprintln!("report  : {}", path.display());
    }
    if let Some(dir) = args.out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("steampipe-style.json"),
            serde_json::to_vec_pretty(&est)?,
        )?;
        std::fs::write(dir.join("steampipe-style.md"), &md)?;
        eprintln!(
            "results : {}/steampipe-style.md + steampipe-style.json",
            dir.display()
        );
    }
    Ok(())
}

fn print_estimate(e: &Estimate) {
    println!(
        "steampipe-style ({} chunks, {}, {} scope): {} → {}",
        human_bytes(e.chunk_size),
        e.compression,
        e.scope,
        e.old_build,
        e.new_build
    );
    println!(
        "build   : {} → {} ({} files, {} unchanged, {} modified, {} new, {} deleted)",
        human_bytes(e.old_size_bytes),
        human_bytes(e.new_size_bytes),
        e.files_new,
        e.files_unchanged,
        e.files_modified,
        e.files_added,
        e.files_deleted,
    );
    println!(
        "chunks  : {} of {} new/changed",
        e.new_or_changed_chunks, e.total_chunks_new
    );
    println!(
        "download: {} estimated ({} raw)",
        human_bytes(e.estimated_download_compressed),
        human_bytes(e.estimated_download_raw)
    );
    println!(
        "local IO: read ~{} + write ~{} to rebuild touched files",
        human_bytes(e.rebuild_read_bytes),
        human_bytes(e.rebuild_write_bytes)
    );
    if let Some(top) = e.largest_contributor() {
        println!(
            "largest : {} ({} of the estimate)",
            top.path,
            human_bytes(top.download_compressed)
        );
    }
    println!("note    : {}", e.note);
}

pub fn estimate_markdown(e: &Estimate) -> String {
    let mut md = String::new();
    md.push_str("# SteamPipe-style Update Estimate\n\n");
    md.push_str(&format!("> {}\n\n", e.note));
    md.push_str("| Metric | Value |\n|---|---:|\n");
    md.push_str(&format!(
        "| Old build size | {} |\n",
        human_bytes(e.old_size_bytes)
    ));
    md.push_str(&format!(
        "| New build size | {} |\n",
        human_bytes(e.new_size_bytes)
    ));
    md.push_str(&format!(
        "| Fixed chunk size | {} |\n",
        human_bytes(e.chunk_size)
    ));
    md.push_str(&format!("| Chunk match scope | {} |\n", e.scope));
    md.push_str(&format!(
        "| New/changed chunks | {} of {} |\n",
        e.new_or_changed_chunks, e.total_chunks_new
    ));
    md.push_str(&format!(
        "| Estimated download | {} ({}) |\n",
        human_bytes(e.estimated_download_compressed),
        e.compression
    ));
    md.push_str(&format!(
        "| Files touched | {} modified + {} new ({} deleted) |\n",
        e.files_modified, e.files_added, e.files_deleted
    ));
    md.push_str(&format!(
        "| Local rebuild I/O | read {} + write {} |\n",
        human_bytes(e.rebuild_read_bytes),
        human_bytes(e.rebuild_write_bytes)
    ));
    if let Some(top) = e.largest_contributor() {
        md.push_str(&format!("| Largest contributor | {} |\n", top.path));
    }
    if !e.files.is_empty() {
        md.push_str("\n## Files by estimated download\n\n");
        md.push_str("| File | Status | Size | Changed chunks | Reuse | Estimated download |\n|---|---|---:|---:|---:|---:|\n");
        for f in e.files.iter().take(25) {
            md.push_str(&format!(
                "| {} | {} | {} | {} of {} | {:.1}% | {} |\n",
                f.path,
                f.status,
                human_bytes(f.new_size),
                f.new_chunks,
                f.total_chunks,
                f.reuse_ratio * 100.0,
                human_bytes(f.download_compressed)
            ));
        }
        if e.files.len() > 25 {
            md.push_str(&format!("\n({} more files omitted)\n", e.files.len() - 25));
        }
    }
    // Warning block for the classic pathology: one pack, many regions.
    for f in e.files.iter().take(3) {
        let scattered = f.changed_chunk_indices.len() >= 64 && f.is_pack;
        if scattered {
            md.push_str(&format!(
                "\n## Warning\n\nThe pack `{}` changed in {} scattered {} regions. \
                 This likely indicates asset shuffling, distributed TOC churn, or \
                 compression across asset boundaries. Run `cavs analyze steampipe` \
                 for a diagnosis.\n",
                f.path,
                f.new_chunks,
                human_bytes(e.chunk_size)
            ));
            break;
        }
    }
    md
}

// ---------------------------------------------------------------------------
// cavs analyze steampipe
// ---------------------------------------------------------------------------

pub struct AnalyzeArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub engine: &'a str,
    pub ignore: Vec<String>,
    pub json: bool,
    pub out: Option<&'a Path>,
}

pub fn analyze_steampipe(args: &AnalyzeArgs) -> Result<()> {
    check_pair(args.old, args.new)?;
    let keep = keeper(args.new, &args.ignore)?;
    let analysis = analyze(
        args.old,
        args.new,
        Engine::parse(args.engine),
        &Thresholds::default(),
        &keep,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&analysis)?);
    } else {
        print_analysis(&analysis);
    }
    if let Some(path) = args.out {
        if path.extension().is_some_and(|e| e == "json") {
            std::fs::write(path, serde_json::to_vec_pretty(&analysis)?)?;
        } else {
            std::fs::write(path, analysis_markdown(&analysis))?;
        }
        eprintln!("report  : {}", path.display());
    }
    Ok(())
}

fn print_analysis(a: &Analysis) {
    println!("analyze steampipe: {} → {}", a.old_build, a.new_build);
    println!(
        "build   : {} → {} ({} modified, {} new, {} deleted, {} unchanged)",
        human_bytes(a.old_size_bytes),
        human_bytes(a.new_size_bytes),
        a.files_modified,
        a.files_added,
        a.files_deleted,
        a.files_unchanged,
    );
    println!(
        "estimate: {} SteamPipe-style vs {} CAVS content-defined",
        human_bytes(a.estimated_steampipe_download),
        human_bytes(a.estimated_cavs_download)
    );
    if a.findings.is_empty() {
        println!("findings: none — changes look localized and chunk-friendly");
    }
    for f in &a.findings {
        println!(
            "  [{}] {}{}",
            f.severity.label().to_uppercase(),
            f.title,
            f.file
                .as_deref()
                .map(|p| format!(" — {p}"))
                .unwrap_or_default()
        );
    }
    println!("note    : {}", a.note);
}

pub fn analysis_markdown(a: &Analysis) -> String {
    let mut md = String::new();
    md.push_str("# SteamPipe-style Build Analysis\n\n");
    md.push_str(&format!("> {}\n\n", a.note));
    md.push_str(&format!(
        "`{}` → `{}` (engine hint: {})\n\n",
        a.old_build, a.new_build, a.engine
    ));
    md.push_str("| Metric | Value |\n|---|---:|\n");
    md.push_str(&format!(
        "| Old build | {} |\n",
        human_bytes(a.old_size_bytes)
    ));
    md.push_str(&format!(
        "| New build | {} |\n",
        human_bytes(a.new_size_bytes)
    ));
    md.push_str(&format!(
        "| SteamPipe-style estimate | {} |\n",
        human_bytes(a.estimated_steampipe_download)
    ));
    md.push_str(&format!(
        "| CAVS content-defined estimate | {} |\n",
        human_bytes(a.estimated_cavs_download)
    ));
    md.push_str(&format!(
        "| Fixed 1 MiB reuse | {:.1}% |\n",
        a.steam_reuse_ratio * 100.0
    ));
    md.push_str(&format!(
        "| Content-defined reuse | {:.1}% |\n",
        a.cdc_reuse_ratio * 100.0
    ));
    md.push_str(&format!(
        "| Files | {} modified, {} new, {} deleted, {} unchanged |\n",
        a.files_modified, a.files_added, a.files_deleted, a.files_unchanged
    ));

    if !a.files.is_empty() {
        md.push_str("\n## Files ranked by update cost\n\n");
        md.push_str(
            "| File | Status | Size | Fixed reuse | CDC reuse | SteamPipe-style | CAVS | Scatteredness |\n|---|---|---:|---:|---:|---:|---:|---:|\n",
        );
        for f in a.files.iter().take(25) {
            md.push_str(&format!(
                "| {} | {} | {} | {:.1}% | {:.1}% | {} | {} | {:.2} |\n",
                f.path,
                f.status,
                human_bytes(f.new_size),
                f.steam_reuse_ratio * 100.0,
                f.cdc_reuse_ratio * 100.0,
                human_bytes(f.steam_download),
                human_bytes(f.cdc_download),
                f.heat_1m.scatteredness,
            ));
        }
    }

    if a.findings.is_empty() {
        md.push_str("\n## Findings\n\nNo major issues detected: changes look localized and chunk-friendly.\n");
    } else {
        md.push_str("\n## Findings\n");
        for f in &a.findings {
            md.push_str(&format!(
                "\n### {}: {}\n\n",
                capitalize(f.severity.label()),
                f.title
            ));
            if let Some(file) = &f.file {
                md.push_str(&format!("File:\n  `{file}`\n\n"));
            }
            if f.estimated_wasted_bytes > 0 {
                md.push_str(&format!(
                    "Estimated wasted bytes: **{}**\n\n",
                    human_bytes(f.estimated_wasted_bytes)
                ));
            }
            md.push_str(&format!("Why it happens:\n  {}\n\n", f.why));
            md.push_str(&format!("Recommended fix:\n  {}\n\n", f.fix));
            md.push_str(&format!(
                "Expected improvement:\n  {}\n",
                f.expected_improvement
            ));
        }
    }
    md
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    #[test]
    fn markdown_labels_the_estimate() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let base = pseudo_random(2 << 20, 3);
        let mut new = base.clone();
        new[0] ^= 1;
        std::fs::write(a.join("x.pak"), &base).unwrap();
        std::fs::write(b.join("x.pak"), &new).unwrap();
        let est = estimate(&a, &b, &ModelConfig::default(), &|_: &str| true).unwrap();
        let md = estimate_markdown(&est);
        assert!(md.contains("not Valve's exact SteamPipe implementation"));
        assert!(md.contains("| Fixed chunk size | 1.00 MiB |"));
    }

    #[test]
    fn mismatched_kinds_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("d");
        std::fs::create_dir_all(&d).unwrap();
        let f = dir.path().join("f.bin");
        std::fs::write(&f, b"x").unwrap();
        assert!(check_pair(&d, &f).is_err());
        let missing = dir.path().join("missing");
        let err = check_pair(&missing, &f).unwrap_err().to_string();
        assert!(err.contains("CAVS-E-ANALYZE-PATH-MISSING"), "{err}");
    }
}
