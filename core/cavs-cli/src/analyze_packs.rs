//! `cavs analyze-packs` and `cavs analyze godot-pck` (v0.9.0).
//!
//! Pack-file layout diagnostics: change heatmaps at 64 KiB / 1 MiB /
//! 8 MiB windows, scatteredness, similarity vs fixed-chunk reuse, TOC
//! churn, compressed-blob detection and size advisories. The Godot
//! variant additionally parses the PCK directory (format v1/v2, when not
//! encrypted) and maps changed byte ranges to the resource paths inside.

use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_analyzer::compare::{analyze, Analysis, FileAnalysis};
use cavs_analyzer::detect::{Finding, Thresholds};
use cavs_analyzer::Engine;
use serde::Serialize;
use std::path::Path;

pub struct PacksArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub engine: &'a str,
    pub out: Option<&'a Path>,
    pub json: bool,
}

#[derive(Serialize)]
struct PackRow<'a> {
    file: &'a str,
    size: u64,
    changed_windows_1m: u64,
    scatteredness: f64,
    entropy_bits: f64,
    fixed_reuse: f64,
    cdc_reuse: f64,
    main_issue: String,
    recommendation: String,
}

#[derive(Serialize)]
struct PackReport<'a> {
    old: String,
    new: String,
    rows: Vec<PackRow<'a>>,
    findings: &'a [Finding],
    note: &'a str,
}

/// The most severe finding attached to a file, if any.
fn main_issue<'a>(a: &'a Analysis, path: &str) -> Option<&'a Finding> {
    a.findings
        .iter()
        .filter(|f| f.file.as_deref() == Some(path))
        .max_by_key(|f| f.severity)
}

fn short_fix(f: &Finding) -> String {
    match f.kind.as_str() {
        "toc_churn" => "centralize TOC / relative offsets".into(),
        "scattered_pack_churn" => "split by level/feature".into(),
        "asset_shuffling" => "keep asset order stable".into(),
        "compressed_blob" => "per-asset compression".into(),
        "oversized_pack" => "split into 1–2 GiB packs".into(),
        "new_content_in_old_pack" => "ship new content as new packs".into(),
        _ => f.fix.chars().take(40).collect(),
    }
}

pub fn analyze_packs(args: &PacksArgs) -> Result<()> {
    let analysis = analyze(
        args.old,
        args.new,
        Engine::parse(args.engine),
        &Thresholds::default(),
        &|_: &str| true,
    )?;

    // Pack files first; when the build has none, fall back to every
    // changed file so single-artifact runs still report.
    let mut files: Vec<&FileAnalysis> = analysis.files.iter().filter(|f| f.is_pack).collect();
    if files.is_empty() {
        files = analysis.files.iter().collect();
    }

    let rows: Vec<PackRow> = files
        .iter()
        .map(|f| {
            let issue = main_issue(&analysis, &f.path);
            PackRow {
                file: &f.path,
                size: f.new_size,
                changed_windows_1m: f.heat_1m.changed_windows,
                scatteredness: f.heat_1m.scatteredness,
                entropy_bits: f.entropy_bits,
                fixed_reuse: f.steam_reuse_ratio,
                cdc_reuse: f.cdc_reuse_ratio,
                main_issue: issue
                    .map(|i| i.kind.clone())
                    .unwrap_or_else(|| "localized".into()),
                recommendation: issue.map(short_fix).unwrap_or_else(|| "OK".into()),
            }
        })
        .collect();

    let report = PackReport {
        old: analysis.old_build.clone(),
        new: analysis.new_build.clone(),
        rows,
        findings: &analysis.findings,
        note: &analysis.note,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("analyze-packs: {} → {}", report.old, report.new);
        for r in &report.rows {
            println!(
                "  {:<40} {:>10}  windows {:>5}  scatter {:>4.2}  {} → {}",
                r.file,
                human_bytes(r.size),
                r.changed_windows_1m,
                r.scatteredness,
                r.main_issue,
                r.recommendation
            );
        }
        if report.rows.is_empty() {
            println!("  no changed files — builds are identical under the model");
        }
    }
    if let Some(path) = args.out {
        std::fs::write(path, markdown(&report))?;
        eprintln!("report  : {}", path.display());
    }
    Ok(())
}

fn markdown(r: &PackReport) -> String {
    let mut md = String::new();
    md.push_str("# Pack Analysis\n\n");
    md.push_str(&format!("> {}\n\n", r.note));
    md.push_str(&format!("`{}` → `{}`\n\n", r.old, r.new));
    md.push_str("| File | Size | Changed windows (1 MiB) | Scatteredness | Entropy | Fixed reuse | CDC reuse | Main issue | Recommendation |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---|---|\n");
    for row in &r.rows {
        md.push_str(&format!(
            "| {} | {} | {} | {:.2} | {:.2} | {:.1}% | {:.1}% | {} | {} |\n",
            row.file,
            human_bytes(row.size),
            row.changed_windows_1m,
            row.scatteredness,
            row.entropy_bits,
            row.fixed_reuse * 100.0,
            row.cdc_reuse * 100.0,
            row.main_issue,
            row.recommendation
        ));
    }
    if !r.findings.is_empty() {
        md.push_str("\n## Findings\n");
        for f in r.findings {
            md.push_str(&format!(
                "\n- **[{}] {}**{} — {}\n",
                f.severity.label(),
                f.title,
                f.file
                    .as_deref()
                    .map(|p| format!(" (`{p}`)"))
                    .unwrap_or_default(),
                f.fix
            ));
        }
    }
    md
}

// ---------------------------------------------------------------------------
// Godot PCK
// ---------------------------------------------------------------------------

pub struct GodotArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub out: Option<&'a Path>,
    pub json: bool,
}

#[derive(Serialize)]
struct GodotReport {
    old: String,
    new: String,
    old_size: u64,
    new_size: u64,
    changed_windows_64k: u64,
    changed_windows_1m: u64,
    scatteredness: f64,
    entropy_bits: f64,
    fixed_reuse: f64,
    cdc_reuse: f64,
    parsed: bool,
    godot_version: Option<String>,
    resources_total: Option<usize>,
    /// Internal resource paths overlapping changed byte ranges.
    changed_resources: Vec<String>,
    findings: Vec<Finding>,
    recommendations: Vec<String>,
    note: String,
}

pub fn analyze_godot_pck(args: &GodotArgs) -> Result<()> {
    for p in [args.old, args.new] {
        if !p.is_file() {
            bail!(
                "CAVS-E-GODOT-PCK-UNSUPPORTED: {} is not a file (pass .pck files)",
                p.display()
            );
        }
    }
    let analysis = analyze(
        args.old,
        args.new,
        Engine::Godot,
        &Thresholds::default(),
        &|_: &str| true,
    )?;

    let (fa, heat64, heat1m) = match analysis.files.first() {
        Some(f) => (f, &f.heat_64k, &f.heat_1m),
        None => {
            println!("godot-pck: files are identical — zero update cost");
            return Ok(());
        }
    };

    // Try to parse the new PCK directory and map changed windows to
    // internal resource paths. Byte-level analysis stands on its own when
    // parsing fails (encrypted directory, unknown version).
    let new_bytes = std::fs::read(args.new)?;
    let parsed = godot_pck::parse(&new_bytes);
    let (godot_version, resources_total, changed_resources) = match &parsed {
        Ok(dir) => {
            let mut touched: Vec<String> = Vec::new();
            for (start_w, len_w) in &heat64.largest_ranges {
                let start = start_w * heat64.window_size;
                let end = (start_w + len_w) * heat64.window_size;
                for entry in &dir.entries {
                    let e_start = entry.offset;
                    let e_end = entry.offset + entry.size;
                    if e_start < end && start < e_end && !touched.contains(&entry.path) {
                        touched.push(entry.path.clone());
                    }
                }
            }
            touched.truncate(50);
            (Some(dir.version.clone()), Some(dir.entries.len()), touched)
        }
        Err(_) => (None, None, Vec::new()),
    };

    let mut recommendations = vec![
        "Keep the base PCK stable; ship frequently updated content in separate PCKs.".into(),
        "Load update/DLC PCKs as resource packs at runtime instead of rewriting the base PCK."
            .into(),
        "Avoid repacking unrelated assets: unchanged resources should keep their offsets.".into(),
    ];
    if fa.cdc_reuse_ratio - fa.steam_reuse_ratio > 0.25 {
        recommendations.insert(
            0,
            "Content survives but offsets moved — keep export order deterministic so \
             resources keep their positions between exports."
                .into(),
        );
    }
    if fa.entropy_bits >= 7.5 {
        recommendations.insert(
            0,
            "The PCK behaves like a compressed blob; prefer per-resource compression \
             so small changes stay local."
                .into(),
        );
    }

    let report = GodotReport {
        old: args.old.display().to_string(),
        new: args.new.display().to_string(),
        old_size: fa.old_size,
        new_size: fa.new_size,
        changed_windows_64k: heat64.changed_windows,
        changed_windows_1m: heat1m.changed_windows,
        scatteredness: heat1m.scatteredness,
        entropy_bits: fa.entropy_bits,
        fixed_reuse: fa.steam_reuse_ratio,
        cdc_reuse: fa.cdc_reuse_ratio,
        parsed: parsed.is_ok(),
        godot_version,
        resources_total,
        changed_resources,
        findings: analysis.findings.clone(),
        recommendations,
        note: analysis.note.clone(),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_godot(&report);
    }
    if let Some(path) = args.out {
        std::fs::write(path, godot_markdown(&report))?;
        eprintln!("report  : {}", path.display());
    }
    Ok(())
}

fn print_godot(r: &GodotReport) {
    println!("godot-pck: {} → {}", r.old, r.new);
    println!(
        "size    : {} → {}",
        human_bytes(r.old_size),
        human_bytes(r.new_size)
    );
    println!(
        "changed : {} × 64 KiB windows, {} × 1 MiB windows (scatteredness {:.2})",
        r.changed_windows_64k, r.changed_windows_1m, r.scatteredness
    );
    println!(
        "reuse   : fixed {:.1}% vs content-defined {:.1}% (entropy {:.2} bits/byte)",
        r.fixed_reuse * 100.0,
        r.cdc_reuse * 100.0,
        r.entropy_bits
    );
    match (r.parsed, &r.godot_version) {
        (true, Some(v)) => {
            println!(
                "pck     : parsed OK (Godot {v}, {} resources)",
                r.resources_total.unwrap_or(0)
            );
            if !r.changed_resources.is_empty() {
                println!("touched : {}", r.changed_resources.join(", "));
            }
        }
        _ => println!("pck     : directory not parseable (encrypted or unknown layout) — byte-level report only"),
    }
    for rec in &r.recommendations {
        println!("  - {rec}");
    }
}

fn godot_markdown(r: &GodotReport) -> String {
    let mut md = String::new();
    md.push_str("# Godot PCK Analysis\n\n");
    md.push_str(&format!("> {}\n\n", r.note));
    md.push_str(&format!("`{}` → `{}`\n\n", r.old, r.new));
    md.push_str("| Metric | Value |\n|---|---:|\n");
    md.push_str(&format!("| Old size | {} |\n", human_bytes(r.old_size)));
    md.push_str(&format!("| New size | {} |\n", human_bytes(r.new_size)));
    md.push_str(&format!(
        "| Changed 64 KiB windows | {} |\n",
        r.changed_windows_64k
    ));
    md.push_str(&format!(
        "| Changed 1 MiB windows | {} |\n",
        r.changed_windows_1m
    ));
    md.push_str(&format!("| Scatteredness | {:.2} |\n", r.scatteredness));
    md.push_str(&format!("| Entropy | {:.2} bits/byte |\n", r.entropy_bits));
    md.push_str(&format!(
        "| Fixed 1 MiB reuse | {:.1}% |\n",
        r.fixed_reuse * 100.0
    ));
    md.push_str(&format!(
        "| Content-defined reuse | {:.1}% |\n",
        r.cdc_reuse * 100.0
    ));
    if r.parsed {
        md.push_str(&format!(
            "| PCK directory | parsed (Godot {}, {} resources) |\n",
            r.godot_version.as_deref().unwrap_or("?"),
            r.resources_total.unwrap_or(0)
        ));
    } else {
        md.push_str("| PCK directory | not parseable — byte-level report |\n");
    }
    if !r.changed_resources.is_empty() {
        md.push_str("\n## Resources overlapping changed regions\n\n");
        for path in &r.changed_resources {
            md.push_str(&format!("- `{path}`\n"));
        }
    }
    md.push_str("\n## Recommendations\n\n");
    for rec in &r.recommendations {
        md.push_str(&format!("- {rec}\n"));
    }
    if !r.findings.is_empty() {
        md.push_str("\n## Findings\n\n");
        for f in &r.findings {
            md.push_str(&format!("- **[{}]** {}\n", f.severity.label(), f.title));
        }
    }
    md
}

/// Minimal, tolerant Godot PCK directory parser (format v1 = Godot 3,
/// v2 = Godot 4; unencrypted directories only).
pub mod godot_pck {
    use anyhow::{bail, Result};

    pub struct Entry {
        pub path: String,
        pub offset: u64,
        pub size: u64,
    }

    pub struct Directory {
        pub version: String,
        pub entries: Vec<Entry>,
    }

    struct Reader<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl<'a> Reader<'a> {
        fn u32(&mut self) -> Result<u32> {
            let end = self.pos + 4;
            if end > self.data.len() {
                bail!("truncated");
            }
            let v = u32::from_le_bytes(self.data[self.pos..end].try_into()?);
            self.pos = end;
            Ok(v)
        }
        fn u64(&mut self) -> Result<u64> {
            let end = self.pos + 8;
            if end > self.data.len() {
                bail!("truncated");
            }
            let v = u64::from_le_bytes(self.data[self.pos..end].try_into()?);
            self.pos = end;
            Ok(v)
        }
        fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
            let end = self.pos + n;
            if end > self.data.len() {
                bail!("truncated");
            }
            let s = &self.data[self.pos..end];
            self.pos = end;
            Ok(s)
        }
    }

    const MAGIC: u32 = 0x4350_4447; // "GDPC"
    const PCK_DIR_ENCRYPTED: u32 = 1;

    pub fn parse(data: &[u8]) -> Result<Directory> {
        let mut r = Reader { data, pos: 0 };
        if r.u32()? != MAGIC {
            bail!("CAVS-E-GODOT-PCK-UNSUPPORTED: no GDPC magic");
        }
        let format = r.u32()?;
        let (major, minor, patch) = (r.u32()?, r.u32()?, r.u32()?);
        let mut file_base = 0u64;
        match format {
            1 => {
                r.bytes(16 * 4)?; // reserved
            }
            2 => {
                let flags = r.u32()?;
                if flags & PCK_DIR_ENCRYPTED != 0 {
                    bail!("CAVS-E-GODOT-PCK-UNSUPPORTED: encrypted directory");
                }
                file_base = r.u64()?;
                r.bytes(16 * 4)?; // reserved
            }
            v => bail!("CAVS-E-GODOT-PCK-UNSUPPORTED: pack format {v}"),
        }
        let count = r.u32()? as usize;
        if count > 1_000_000 {
            bail!("CAVS-E-GODOT-PCK-UNSUPPORTED: implausible entry count");
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let path_len = r.u32()? as usize;
            if path_len > 64 * 1024 {
                bail!("CAVS-E-GODOT-PCK-UNSUPPORTED: implausible path length");
            }
            let raw = r.bytes(path_len)?;
            let path = String::from_utf8_lossy(raw)
                .trim_end_matches('\0')
                .to_string();
            let mut offset = r.u64()?;
            let size = r.u64()?;
            r.bytes(16)?; // md5
            if format == 2 {
                let _flags = r.u32()?;
                offset += file_base;
            }
            entries.push(Entry { path, offset, size });
        }
        Ok(Directory {
            version: format!("{major}.{minor}.{patch} (pack format {format})"),
            entries,
        })
    }

    /// Build a synthetic, well-formed PCK for tests and the
    /// `cavs bench steampipe-cases` Godot cases.
    pub fn synth(format: u32, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&MAGIC.to_le_bytes());
        header.extend_from_slice(&format.to_le_bytes());
        for v in [4u32, 2, 0] {
            header.extend_from_slice(&v.to_le_bytes());
        }
        if format == 2 {
            header.extend_from_slice(&0u32.to_le_bytes()); // flags
            header.extend_from_slice(&0u64.to_le_bytes()); // file_base
        }
        header.extend_from_slice(&[0u8; 16 * 4]);
        header.extend_from_slice(&(files.len() as u32).to_le_bytes());

        // Compute the directory size to place payloads after it.
        let mut dir_size = 0usize;
        for (path, _) in files {
            dir_size += 4 + path.len() + 8 + 8 + 16 + if format == 2 { 4 } else { 0 };
        }
        let payload_start = header.len() + dir_size;
        let mut dir = Vec::new();
        let mut payloads = Vec::new();
        let mut offset = payload_start as u64;
        for (path, bytes) in files {
            dir.extend_from_slice(&(path.len() as u32).to_le_bytes());
            dir.extend_from_slice(path.as_bytes());
            dir.extend_from_slice(&offset.to_le_bytes());
            dir.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            dir.extend_from_slice(&[0u8; 16]);
            if format == 2 {
                dir.extend_from_slice(&0u32.to_le_bytes());
            }
            payloads.extend_from_slice(bytes);
            offset += bytes.len() as u64;
        }
        header.extend_from_slice(&dir);
        header.extend_from_slice(&payloads);
        header
    }
}

#[cfg(test)]
mod tests {
    use super::godot_pck;

    #[test]
    fn synthetic_pck_round_trips_both_formats() {
        for format in [1u32, 2] {
            let pck = godot_pck::synth(
                format,
                &[
                    ("res://scenes/main.tscn", b"scene data".as_slice()),
                    ("res://textures/hero.png", &[7u8; 4096]),
                ],
            );
            let dir = godot_pck::parse(&pck).unwrap();
            assert_eq!(dir.entries.len(), 2, "format {format}");
            assert_eq!(dir.entries[0].path, "res://scenes/main.tscn");
            let e = &dir.entries[1];
            assert_eq!(
                &pck[e.offset as usize..(e.offset + e.size) as usize],
                &[7u8; 4096]
            );
        }
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(godot_pck::parse(b"not a pck").is_err());
        let err = match godot_pck::parse(&[0u8; 64]) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("zeroed buffer must not parse"),
        };
        assert!(err.contains("CAVS-E-GODOT-PCK-UNSUPPORTED"), "{err}");
    }
}
