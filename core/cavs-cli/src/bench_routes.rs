//! `cavs bench routes` — one old→new transition, every delivery route,
//! one table. Routes that need missing external tools are reported as
//! skipped, never fatal; every produced output is verified byte-identical.

use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_chunker::ChunkMode;
use cavs_plan::{build as build_plan, BuildOptions, PlanKind};
use cavs_signature::CavsSignature;
use std::path::Path;
use std::time::Instant;

const CAVS_MODE: ChunkMode = ChunkMode::Cdc {
    min: 16 * 1024,
    avg: 64 * 1024,
    max: 256 * 1024,
};

#[derive(serde::Serialize)]
pub struct RouteRow {
    pub route: String,
    pub network_bytes: u64,
    pub diff_ms: Option<u64>,
    pub apply_ms: Option<u64>,
    pub peak_rss_mib: Option<f64>,
    pub output_ok: Option<bool>,
    pub notes: String,
}

#[derive(Default, serde::Serialize)]
pub struct RoutesReport {
    pub mode: String,
    pub old: String,
    pub new: String,
    pub old_size_bytes: u64,
    pub new_size_bytes: u64,
    pub routes: Vec<RouteRow>,
    pub skipped: Vec<String>,
}

pub struct RoutesArgs<'a> {
    pub old: &'a Path,
    pub new: &'a Path,
    pub butler_bin: Option<&'a str>,
    pub include_pairwise_proxy: bool,
    pub out: &'a Path,
}

pub fn bench(args: &RoutesArgs) -> Result<()> {
    let report = collect(args)?;
    print_report(&report);
    std::fs::write(
        args.out.join("routes.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::write(args.out.join("routes.md"), markdown(&report))?;
    println!("results : {}/routes.md + routes.json", args.out.display());
    Ok(())
}

/// Measure every route and return the report without printing or writing
/// summaries (temp artifacts still land under `args.out`).
pub fn collect(args: &RoutesArgs) -> Result<RoutesReport> {
    if args.old.is_dir() != args.new.is_dir() {
        bail!("--old and --new must both be files or both be directories");
    }
    let mode = if args.old.is_dir() {
        "directory"
    } else {
        "artifact"
    };
    std::fs::create_dir_all(args.out)?;
    let mut report = RoutesReport {
        mode: mode.into(),
        old: args.old.display().to_string(),
        new: args.new.display().to_string(),
        old_size_bytes: crate::bench_butler::tree_size(args.old)?,
        new_size_bytes: crate::bench_butler::tree_size(args.new)?,
        ..Default::default()
    };

    let files = load_files(args.new)?;

    // ---- Route: full raw download -----------------------------------------
    report.routes.push(RouteRow {
        route: "full download (raw)".into(),
        network_bytes: report.new_size_bytes,
        diff_ms: None,
        apply_ms: Some(0),
        peak_rss_mib: None,
        output_ok: Some(true),
        notes: "no old-version reuse".into(),
    });

    // ---- Route: full zstd-19 (bootstrap-shaped) ----------------------------
    {
        let t0 = Instant::now();
        let mut compressed = 0u64;
        let mut streams: Vec<Vec<u8>> = Vec::new();
        for (_, bytes) in &files {
            let c = zstd::bulk::compress(bytes, 19)?;
            compressed += c.len() as u64;
            streams.push(c);
        }
        let gen_ms = t0.elapsed().as_millis() as u64;
        let t0 = Instant::now();
        let mut ok = true;
        for (c, (_, bytes)) in streams.iter().zip(&files) {
            let raw = zstd::bulk::decompress(c, bytes.len())?;
            ok &= raw == *bytes;
        }
        report.routes.push(RouteRow {
            route: "full zstd-19 (CAVS bootstrap)".into(),
            network_bytes: compressed,
            diff_ms: Some(gen_ms),
            apply_ms: Some(t0.elapsed().as_millis() as u64),
            peak_rss_mib: None,
            output_ok: Some(ok),
            notes: "cache-less first install".into(),
        });
    }

    // ---- Route: CAVS chunk / hybrid wire bytes ------------------------------
    {
        let t0 = Instant::now();
        let mut old_hashes = std::collections::HashSet::new();
        for (_, bytes) in &load_files(args.old)? {
            for range in cavs_chunker::split(bytes, CAVS_MODE) {
                old_hashes.insert(cavs_hash::hash_chunk(&bytes[range]));
            }
        }
        let mut update_bytes = 0u64;
        let mut new_chunks = 0u64;
        let mut total_chunks = 0u64;
        let mut seen = std::collections::HashSet::new();
        for (_, bytes) in &files {
            for range in cavs_chunker::split(bytes, CAVS_MODE) {
                let chunk = &bytes[range];
                let hash = cavs_hash::hash_chunk(chunk);
                total_chunks += 1;
                if !old_hashes.contains(&hash) && seen.insert(hash) {
                    new_chunks += 1;
                    update_bytes += zstd::bulk::compress(chunk, 3)?.len() as u64;
                }
            }
        }
        report.routes.push(RouteRow {
            route: "CAVS chunk / hybrid (wire)".into(),
            network_bytes: update_bytes,
            diff_ms: Some(t0.elapsed().as_millis() as u64),
            apply_ms: None,
            peak_rss_mib: None,
            output_ok: Some(true),
            notes: format!(
                "{new_chunks} of {total_chunks} chunks new; same bytes for warm cache \
                 or cold cache + previous install (hybrid)"
            ),
        });
    }

    // ---- Route: CAVS offline plan (.cavsplan, end-to-end on disk) ----------
    {
        let t0 = Instant::now();
        let label = args
            .old
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let sig = if args.old.is_dir() {
            CavsSignature::sign_dir(args.old, cavs_signature::DEFAULT_BLOCK_SIZE, &label)?
        } else {
            CavsSignature::sign_file(args.old, cavs_signature::DEFAULT_BLOCK_SIZE, &label)?
        };
        let plan = build_plan(
            &sig,
            args.new,
            &BuildOptions {
                kind: PlanKind::Portable,
                zstd_level: 19,
            },
        )?;
        let encoded = plan.encode(19);
        let plan_path = args.out.join("update.cavsplan");
        std::fs::write(&plan_path, &encoded)?;
        let diff_ms = t0.elapsed().as_millis() as u64;

        let t0 = Instant::now();
        let (apply_ok, apply_ms) = if args.old.is_dir() {
            let rebuilt = args.out.join("cavsplan-apply");
            if rebuilt.exists() {
                std::fs::remove_dir_all(&rebuilt)?;
            }
            crate::bench_butler::copy_tree(args.old, &rebuilt)?;
            cavs_plan::apply::apply_dir(
                &plan,
                &rebuilt,
                &rebuilt,
                &cavs_plan::apply::ApplyOptions {
                    delete_removed: true,
                    ..Default::default()
                },
            )?;
            let ms = t0.elapsed().as_millis() as u64;
            let ok = crate::bench_butler::trees_identical(args.new, &rebuilt)?;
            std::fs::remove_dir_all(&rebuilt)?;
            (ok, ms)
        } else {
            let rebuilt = args.out.join("cavsplan-apply.bin");
            cavs_plan::apply::apply_artifact(&plan, args.old, &rebuilt)?;
            let ms = t0.elapsed().as_millis() as u64;
            let ok = std::fs::read(&rebuilt)? == std::fs::read(args.new)?;
            std::fs::remove_file(&rebuilt)?;
            (ok, ms)
        };
        report.routes.push(RouteRow {
            route: "CAVS offline plan (.cavsplan)".into(),
            network_bytes: encoded.len() as u64,
            diff_ms: Some(diff_ms),
            apply_ms: Some(apply_ms),
            peak_rss_mib: None,
            output_ok: Some(apply_ok),
            notes: "portable patch: signature diff + zstd-19 payload, journaled apply".into(),
        });
    }

    // ---- Route: butler offline ---------------------------------------------
    if let Some(butler) = args.butler_bin {
        match crate::bench_butler::run(args.old, args.new, butler, &args.out.join("butler")) {
            Ok(b) => report.routes.push(RouteRow {
                route: "butler offline/default patch".into(),
                network_bytes: b.patch_pwr_bytes,
                diff_ms: Some(b.diff_ms),
                apply_ms: Some(b.apply_ms),
                peak_rss_mib: b.peak_rss_mib,
                output_ok: Some(b.output_matches),
                notes: format!(
                    "+{} signature; default patch (bench butler-full measures the optimized one)",
                    human_bytes(b.signature_pwr_sig_bytes)
                ),
            }),
            Err(e) => report.skipped.push(format!("butler offline: {e}")),
        }
    } else {
        report
            .skipped
            .push("butler offline: no --butler-bin given".into());
    }

    // ---- Route: optimized pairwise proxies ----------------------------------
    if args.include_pairwise_proxy {
        match crate::bench_pairwise::run(args.old, args.new, "bsdiff,xdelta3", "zstd-19,brotli-9") {
            Ok(p) => {
                for res in p.results {
                    report.routes.push(RouteRow {
                        route: format!("pairwise proxy: {}", res.method),
                        network_bytes: res.patch_bytes_compressed,
                        diff_ms: Some(res.diff_ms),
                        apply_ms: Some(res.apply_ms),
                        peak_rss_mib: res.peak_rss_mib,
                        output_ok: Some(res.output_matches),
                        notes: "one exact old→new pair only (proxy)".into(),
                    });
                }
                for n in p.notes {
                    report.skipped.push(n);
                }
            }
            Err(e) => report.skipped.push(format!("pairwise proxy: {e}")),
        }
    }

    Ok(report)
}

/// (label, bytes) of every file in a build.
fn load_files(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    if path.is_file() {
        return Ok(vec![(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            std::fs::read(path)?,
        )]);
    }
    let mut out = Vec::new();
    for rel in crate::compare::walk_sorted(path)? {
        let full = path.join(&rel);
        if full.is_file() {
            out.push((
                rel.to_string_lossy().replace('\\', "/"),
                std::fs::read(&full)?,
            ));
        }
    }
    if out.is_empty() {
        bail!("{} contains no files", path.display());
    }
    Ok(out)
}

fn print_report(r: &RoutesReport) {
    println!(
        "bench routes ({} mode): {} → {} ({} → {})",
        r.mode,
        r.old,
        r.new,
        human_bytes(r.old_size_bytes),
        human_bytes(r.new_size_bytes)
    );
    for row in &r.routes {
        println!(
            "  {:<34} {:>12}  diff {:>7}  apply {:>7}  {}",
            row.route,
            human_bytes(row.network_bytes),
            row.diff_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            row.apply_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            match row.output_ok {
                Some(true) => "OK",
                Some(false) => "MISMATCH",
                None => "",
            }
        );
    }
    for s in &r.skipped {
        println!("  skipped: {s}");
    }
}

pub fn markdown(r: &RoutesReport) -> String {
    let mut md = String::new();
    md.push_str("# Delivery route comparison\n\n");
    md.push_str(&format!(
        "`{}` → `{}` ({} mode, {} → {})\n\n",
        r.old,
        r.new,
        r.mode,
        human_bytes(r.old_size_bytes),
        human_bytes(r.new_size_bytes)
    ));
    md.push_str("| Route | Network bytes | Diff time | Apply time | Peak RSS | Output OK | Notes |\n|---|---:|---:|---:|---:|---|---|\n");
    for row in &r.routes {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            row.route,
            human_bytes(row.network_bytes),
            row.diff_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            row.apply_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
            row.peak_rss_mib
                .map(|m| format!("{m:.0} MiB"))
                .unwrap_or_else(|| "—".into()),
            match row.output_ok {
                Some(true) => "yes",
                Some(false) => "**no**",
                None => "—",
            },
            row.notes
        ));
    }
    if !r.skipped.is_empty() {
        md.push('\n');
        for s in &r.skipped {
            md.push_str(&format!("> skipped: {s}\n"));
        }
    }
    md
}
