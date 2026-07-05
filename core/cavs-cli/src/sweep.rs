//! `cavs sweep` — measure candidate chunk profiles on a real payload (and
//! optionally its previous version) and report which one is cheapest under
//! the cold-install and live-update cost weights.

use crate::classify;
use crate::profile::{self, ChunkProfile, CostWeights, ProfileEstimate};
use anyhow::{Context, Result};
use std::path::Path;

pub fn sweep(
    new: &Path,
    prev: Option<&Path>,
    profiles: Option<&str>,
    zstd_level: i32,
    json_out: Option<&Path>,
) -> Result<()> {
    let data = std::fs::read(new).with_context(|| format!("cannot read {}", new.display()))?;
    let prev_data = match prev {
        Some(p) => Some(profile::load_prev(p)?),
        None => None,
    };

    let payload = classify::classify(new, &data);
    eprintln!(
        "[sweep] payload: {} · entropy {:.2} bits/B · zstd probe ratio {:.3}{}{}",
        payload.kind.label(),
        payload.entropy_score,
        payload.zstd_sample_ratio,
        if payload.likely_precompressed {
            " · precompressed"
        } else {
            ""
        },
        if payload.likely_update_heavy {
            " · update-heavy"
        } else {
            ""
        },
    );

    let candidates: Vec<ChunkProfile> = match profiles {
        Some(list) => list
            .split(',')
            .map(|s| ChunkProfile::parse(s.trim()))
            .collect::<Result<_>>()?,
        None => payload.recommended_profiles.clone(),
    };

    let estimates: Vec<ProfileEstimate> = candidates
        .iter()
        .map(|&p| profile::estimate(&data, prev_data.as_ref(), p, zstd_level))
        .collect();

    let cold = CostWeights::cold_install();
    let update = CostWeights::live_updates();

    println!(
        "{:<14} {:>8} {:>12} {:>12} {:>12} {:>10} {:>8} {:>7}",
        "profile", "chunks", "storage", "cold", "update", "manifest", "reuse%", "enc ms"
    );
    for e in &estimates {
        println!(
            "{:<14} {:>8} {:>12} {:>12} {:>12} {:>10} {:>7.1}% {:>7}",
            e.profile.label(),
            e.chunk_count,
            human(e.storage_bytes),
            human(e.cold_egress_bytes),
            human(e.update_egress_bytes),
            human(e.manifest_bytes),
            e.reuse_ratio * 100.0,
            e.encode_ms,
        );
    }

    let best_cold = profile::choose_best(&estimates, &cold);
    let best_update = profile::choose_best(&estimates, &update);
    println!("best for cold install : {}", best_cold.label());
    println!("best for live updates : {}", best_update.label());

    if let Some(path) = json_out {
        let rows: Vec<serde_json::Value> = estimates
            .iter()
            .map(|e| {
                serde_json::json!({
                    "profile": e.profile.label(),
                    "chunk_count": e.chunk_count,
                    "storage_bytes": e.storage_bytes,
                    "cold_egress_bytes": e.cold_egress_bytes,
                    "update_egress_bytes": e.update_egress_bytes,
                    "manifest_bytes": e.manifest_bytes,
                    "request_count": e.request_count,
                    "encode_ms": e.encode_ms,
                    "reuse_ratio": e.reuse_ratio,
                    "score_cold": profile::score(e, &cold),
                    "score_update": profile::score(e, &update),
                })
            })
            .collect();
        let doc = serde_json::json!({
            "input": new.display().to_string(),
            "prev": prev.map(|p| p.display().to_string()),
            "payload_kind": payload.kind.label(),
            "entropy": payload.entropy_score,
            "zstd_sample_ratio": payload.zstd_sample_ratio,
            "best_cold": best_cold.label(),
            "best_update": best_update.label(),
            "profiles": rows,
        });
        std::fs::write(path, serde_json::to_string_pretty(&doc)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        eprintln!("[sweep] wrote {}", path.display());
    }
    Ok(())
}

fn human(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
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
