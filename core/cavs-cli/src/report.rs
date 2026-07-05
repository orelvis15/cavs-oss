//! Inspection (`info`) and verification (`verify`) reporting.

use anyhow::{Context, Result};
use cavs_format::{PackStats, Reader};
use cavs_hash::to_hex;
use std::path::Path;

pub fn human_bytes(n: u64) -> String {
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

pub fn print_pack_stats(output: &Path, s: &PackStats) {
    let dedup_saved = s.logical_raw.saturating_sub(s.unique_raw);
    let compress_saved = s.unique_raw.saturating_sub(s.stored);
    println!("packed  : {}", output.display());
    println!("file    : {}", human_bytes(s.file_size));
    println!(
        "chunks  : {} unique / {} logical",
        s.unique_chunks, s.logical_chunks
    );
    println!(
        "payload : {} logical -> {} unique -> {} stored",
        human_bytes(s.logical_raw),
        human_bytes(s.unique_raw),
        human_bytes(s.stored)
    );
    println!(
        "savings : dedup {} ({:.2}%), compression {} ({:.2}%), total {:.2}%",
        human_bytes(dedup_saved),
        percent(dedup_saved, s.logical_raw),
        human_bytes(compress_saved),
        percent(compress_saved, s.unique_raw),
        percent(s.logical_raw.saturating_sub(s.stored), s.logical_raw),
    );
    println!("merkle  : {}", to_hex(&s.merkle_root));
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

pub fn info(input: &Path, list_segments: bool, list_chunks: bool) -> Result<()> {
    let r = Reader::open(input).with_context(|| format!("cannot open {}", input.display()))?;
    let sb = r.superblock();

    println!("file      : {}", input.display());
    println!(
        "format    : CAVS-{}.{}  (hash algo {}, compression {})",
        sb.version_major,
        sb.version_minor,
        match sb.hash_algo {
            1 => "BLAKE3-256".to_string(),
            v => format!("#{v}"),
        },
        match sb.compression_algo {
            0 => "none".to_string(),
            1 => "zstd".to_string(),
            v => format!("#{v}"),
        }
    );
    println!(
        "asset uuid: {}",
        sb.asset_uuid
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    println!("file size : {}", human_bytes(sb.file_size));

    println!("\nsections:");
    for s in r.sections() {
        println!(
            "  {:<10} offset {:>12}  length {:>12}",
            format!("{:?}", s.section_type),
            s.offset,
            human_bytes(s.length)
        );
    }

    println!("\ntracks:");
    for t in r.tracks() {
        println!(
            "  #{:<4} {:<8} codec={:<12} name={:<24} init_chunks={} segments={}",
            t.track_id,
            t.kind.label(),
            t.codec,
            t.name,
            t.init_chunks.len(),
            r.segments_for_track(t.track_id).len()
        );
    }

    let integ = r.integrity();
    // Logical bytes = what reconstruction would emit (all segment + init refs).
    let chunk_raw = |idx: u32| r.chunks()[idx as usize].len_raw as u64;
    let mut logical: u64 = 0;
    for t in r.tracks() {
        logical += t.init_chunks.iter().map(|&c| chunk_raw(c)).sum::<u64>();
    }
    for s in r.segments() {
        logical += s.chunks.iter().map(|&c| chunk_raw(c)).sum::<u64>();
    }

    println!("\npayload:");
    println!("  segments        : {}", r.segments().len());
    println!("  dict (pinned)   : {} chunks", r.dict().len());
    println!("  unique chunks   : {}", integ.chunk_count);
    println!("  logical bytes   : {}", human_bytes(logical));
    println!("  unique bytes    : {}", human_bytes(integ.total_raw));
    println!("  stored bytes    : {}", human_bytes(integ.total_stored));
    println!(
        "  dedup saving    : {:.2}%",
        percent(logical.saturating_sub(integ.total_raw), logical)
    );
    println!(
        "  compress saving : {:.2}%",
        percent(
            integ.total_raw.saturating_sub(integ.total_stored),
            integ.total_raw
        )
    );
    println!("  merkle root     : {}", to_hex(&integ.merkle_root));

    if !r.meta().is_empty() {
        println!("\nmeta:");
        for (k, v) in r.meta() {
            let shown: String = v.chars().take(80).collect();
            println!("  {k} = {shown}{}", if v.len() > 80 { "..." } else { "" });
        }
    }

    if list_segments {
        println!("\nsegments:");
        for s in r.segments() {
            let bytes: u64 = s.chunks.iter().map(|&c| chunk_raw(c)).sum();
            println!(
                "  id {:<6} track {:<5} pts {:>10} dur {:>7} chunks {:>4}  {}",
                s.segment_id,
                s.track_id,
                s.pts_start,
                s.duration,
                s.chunks.len(),
                human_bytes(bytes)
            );
        }
    }

    if list_chunks {
        println!("\nchunks:");
        for (i, c) in r.chunks().iter().enumerate() {
            println!(
                "  {:<6} {}  raw {:>10} stored {:>10} flags {:#x}",
                i,
                to_hex(&c.hash),
                c.len_raw,
                c.len_stored,
                c.flags
            );
        }
    }

    Ok(())
}

pub fn verify(input: &Path, pubkey: Option<&str>) -> Result<()> {
    let mut r = Reader::open(input).with_context(|| format!("cannot open {}", input.display()))?;
    let report = r.verify()?;
    println!(
        "OK: {} chunks verified ({}), merkle root and DATA section hash match",
        report.chunks_verified,
        human_bytes(report.bytes_verified)
    );

    let status = r.verify_signature()?;
    match (&status, pubkey) {
        (cavs_format::SignatureStatus::Unsigned, None) => {
            println!("signature: none embedded");
        }
        (cavs_format::SignatureStatus::Unsigned, Some(_)) => {
            anyhow::bail!("--pubkey given but the file has no content signature");
        }
        (cavs_format::SignatureStatus::Valid(pk), expected) => {
            let pk_hex: String = pk.iter().map(|b| format!("{b:02x}")).collect();
            if let Some(exp) = expected {
                // Accept a literal hex key or a path to a .pub file.
                let exp_hex = if exp.len() == 64 && exp.chars().all(|c| c.is_ascii_hexdigit()) {
                    exp.to_string()
                } else {
                    std::fs::read_to_string(exp)
                        .with_context(|| format!("cannot read pubkey file {exp}"))?
                        .trim()
                        .to_string()
                };
                if !pk_hex.eq_ignore_ascii_case(&exp_hex) {
                    anyhow::bail!(
                        "signature valid but signer {pk_hex} does not match expected {exp_hex}"
                    );
                }
                println!("signature: VALID and signer matches ({pk_hex})");
            } else {
                println!("signature: valid (signer {pk_hex})");
            }
        }
    }
    Ok(())
}
