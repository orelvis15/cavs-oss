//! `cavs bench compression` (v0.6.0) — measure zstd vs Brotli on a real
//! payload without changing any default. Wharf recommends Brotli (q1
//! transport, q9 storage); CAVS ships zstd-3 — this harness exists so that
//! choice stays backed by numbers, per payload class.
//!
//! Brotli support is feature-gated (`--features brotli-bench`); without it
//! the brotli algos are reported as skipped.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Instant;

#[derive(serde::Serialize)]
struct AlgoResult {
    algo: String,
    compressed_bytes: u64,
    encode_ms: f64,
    decode_ms: f64,
    ratio: f64,
}

pub fn bench(input: &Path, algos: &str, out: Option<&Path>) -> Result<()> {
    let data = std::fs::read(input).with_context(|| format!("cannot read {}", input.display()))?;
    if data.is_empty() {
        bail!("{} is empty", input.display());
    }
    let mut results = Vec::new();
    let mut skipped = Vec::new();
    for algo in algos.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match run_algo(algo, &data) {
            Ok(Some(r)) => results.push(r),
            Ok(None) => skipped.push(algo.to_string()),
            Err(e) => bail!("algo {algo}: {e}"),
        }
    }

    println!(
        "bench compression: {} ({})",
        input.display(),
        human(data.len() as u64)
    );
    println!("| algo | size | ratio | encode ms | decode ms |");
    println!("|---|---:|---:|---:|---:|");
    for r in &results {
        println!(
            "| {} | {} | {:.3} | {:.0} | {:.0} |",
            r.algo,
            human(r.compressed_bytes),
            r.ratio,
            r.encode_ms,
            r.decode_ms
        );
    }
    for s in &skipped {
        println!("| {s} | skipped (build with --features brotli-bench) | | | |");
    }

    if let Some(path) = out {
        let mut md = format!(
            "# Compression benchmark — {} ({})\n\n| algo | size | ratio | encode ms | decode ms |\n|---|---:|---:|---:|---:|\n",
            input.display(),
            human(data.len() as u64)
        );
        for r in &results {
            md.push_str(&format!(
                "| {} | {} | {:.3} | {:.0} | {:.0} |\n",
                r.algo,
                human(r.compressed_bytes),
                r.ratio,
                r.encode_ms,
                r.decode_ms
            ));
        }
        md.push_str("\nDefault stays zstd-3 unless a class of payloads proves otherwise.\n");
        std::fs::write(path, md)?;
        println!("report  : {}", path.display());
    }
    Ok(())
}

/// Returns Ok(None) when the algorithm is not compiled in.
fn run_algo(algo: &str, data: &[u8]) -> Result<Option<AlgoResult>> {
    let (family, level) = algo
        .rsplit_once('-')
        .with_context(|| format!("bad algo {algo:?}; expected e.g. zstd-3 or brotli-9"))?;
    let level: i32 = level
        .parse()
        .with_context(|| format!("bad level in {algo:?}"))?;
    match family {
        "zstd" => {
            let t0 = Instant::now();
            let compressed = zstd::bulk::compress(data, level)?;
            let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let t0 = Instant::now();
            let round = zstd::bulk::decompress(&compressed, data.len())?;
            let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;
            if round != data {
                bail!("zstd roundtrip mismatch");
            }
            Ok(Some(AlgoResult {
                algo: algo.to_string(),
                compressed_bytes: compressed.len() as u64,
                encode_ms,
                decode_ms,
                ratio: compressed.len() as f64 / data.len() as f64,
            }))
        }
        "brotli" => brotli_algo(algo, level, data),
        other => bail!("unknown compression family {other:?}"),
    }
}

#[cfg(feature = "brotli-bench")]
fn brotli_algo(algo: &str, level: i32, data: &[u8]) -> Result<Option<AlgoResult>> {
    use std::io::{Read as _, Write as _};
    let t0 = Instant::now();
    let mut compressed = Vec::new();
    {
        let mut w =
            brotli::CompressorWriter::new(&mut compressed, 1 << 16, level.max(0) as u32, 22);
        w.write_all(data)?;
        w.flush()?;
    }
    let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t0 = Instant::now();
    let mut round = Vec::with_capacity(data.len());
    brotli::Decompressor::new(compressed.as_slice(), 1 << 16).read_to_end(&mut round)?;
    let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if round != data {
        bail!("brotli roundtrip mismatch");
    }
    Ok(Some(AlgoResult {
        algo: algo.to_string(),
        compressed_bytes: compressed.len() as u64,
        encode_ms,
        decode_ms,
        ratio: compressed.len() as f64 / data.len() as f64,
    }))
}

#[cfg(not(feature = "brotli-bench"))]
fn brotli_algo(_algo: &str, _level: i32, _data: &[u8]) -> Result<Option<AlgoResult>> {
    Ok(None)
}

fn human(n: u64) -> String {
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
