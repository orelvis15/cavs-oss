//! `cavs manifest` — export and benchmark manifest formats (v0.3.0).
//!
//! `export` produces the human-readable JSON v1 manifest of a container
//! (the debug/compatibility view of the compact binary v2 wire format).
//! `bench` encodes the same runtime manifest both ways and reports size,
//! parse time and bytes per logical chunk, so format regressions show up
//! as numbers instead of surprises.

use anyhow::{Context, Result};
use cavs_manifest::{encode_manifest_v2, manifest_from_reader, read_manifest};
use std::path::Path;
use std::time::Instant;

/// Parse iterations for the timing comparison; the best run is reported
/// to shave scheduler noise.
const BENCH_ITERATIONS: u32 = 20;

fn load_manifest(input: &Path) -> Result<cavs_proto::Manifest> {
    let asset_name = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "asset".to_string());
    let reader = cavs_format::Reader::open(input)
        .with_context(|| format!("cannot open {}", input.display()))?;
    manifest_from_reader(&reader, &asset_name).context("building manifest")
}

/// Write the readable JSON v1 manifest to `--out` (or stdout).
pub fn export(input: &Path, out: Option<&Path>) -> Result<()> {
    let manifest = load_manifest(input)?;
    let json = serde_json::to_string_pretty(&manifest)?;
    match out {
        Some(path) => {
            std::fs::write(path, &json)
                .with_context(|| format!("cannot write {}", path.display()))?;
            eprintln!("manifest exported to {}", path.display());
        }
        None => println!("{json}"),
    }
    Ok(())
}

/// Compare JSON v1 and binary v2 for the same runtime manifest.
pub fn bench(input: &Path, json_out: Option<&Path>) -> Result<()> {
    let manifest = load_manifest(input)?;

    let v1_bytes = serde_json::to_vec(&manifest)?;
    let v2_bytes = encode_manifest_v2(&manifest).context("encoding binary v2")?;

    let best_parse_ms = |bytes: &[u8]| -> Result<f64> {
        let mut best = f64::INFINITY;
        for _ in 0..BENCH_ITERATIONS {
            let started = Instant::now();
            let loaded = read_manifest(bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&loaded);
            best = best.min(elapsed);
        }
        Ok(best)
    };
    let v1_parse_ms = best_parse_ms(&v1_bytes)?;
    let v2_parse_ms = best_parse_ms(&v2_bytes)?;

    let logical: u64 = manifest
        .tracks
        .iter()
        .map(|t| t.init_chunks.len() as u64)
        .chain(manifest.segments.iter().map(|s| s.chunks.len() as u64))
        .sum();
    let unique = manifest.chunk_table.len() as u64;
    let savings = if v1_bytes.is_empty() {
        0.0
    } else {
        (v1_bytes.len().saturating_sub(v2_bytes.len())) as f64 * 100.0 / v1_bytes.len() as f64
    };
    let per_chunk = if logical == 0 {
        0.0
    } else {
        v2_bytes.len() as f64 / logical as f64
    };

    println!("Manifest benchmark: {}", input.display());
    println!("  chunks           : {logical} logical / {unique} unique");
    println!(
        "  json-v1          : {} wire, parse {:.3} ms",
        human_bytes(v1_bytes.len() as u64),
        v1_parse_ms
    );
    println!(
        "  binary-v2        : {} wire, parse {:.3} ms",
        human_bytes(v2_bytes.len() as u64),
        v2_parse_ms
    );
    println!("  savings          : {savings:.1}% smaller than json-v1");
    println!("  bytes per chunk  : {per_chunk:.1} B wire (binary-v2, logical)");

    if let Some(path) = json_out {
        let report = format!(
            "{{\"asset\":{},\"chunk_count_logical\":{},\"chunk_count_unique\":{},\"json_v1\":{{\"wire_bytes\":{},\"parse_ms\":{:.3}}},\"binary_v2\":{{\"wire_bytes\":{},\"parse_ms\":{:.3}}},\"savings_pct\":{:.2},\"bytes_per_logical_chunk\":{:.2}}}",
            serde_json::to_string(&manifest.asset)?,
            logical,
            unique,
            v1_bytes.len(),
            v1_parse_ms,
            v2_bytes.len(),
            v2_parse_ms,
            savings,
            per_chunk
        );
        std::fs::write(path, report).with_context(|| format!("cannot write {}", path.display()))?;
        eprintln!("bench report written to {}", path.display());
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
