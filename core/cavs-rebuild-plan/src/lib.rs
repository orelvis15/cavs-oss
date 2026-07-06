//! Hybrid reconstruction plans (v0.6.0).
//!
//! A [`ReconstructionPlan`] describes how to rebuild one output artifact
//! from every available source: chunks in the local cache, verified ranges
//! of a previously installed artifact, and chunks fetched from the network.
//! The old v0.5 behaviour (cache + network only) is expressible as a plan
//! with no `CopyPreviousRange` ops, so the plan executor is a superset of
//! the previous reconstruction path, not a parallel one.
//!
//! Planning is deterministic: the same inputs always produce the same plan.
//! Hashes travel as lowercase hex so plans serialize directly to JSON for
//! `--dump-plan` and stats reporting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Coalesced previous-artifact reads never exceed this (keeps peak RAM and
/// read latency bounded; mirrors the packfile read coalescing of v0.4.0).
pub const MAX_COALESCED_RANGE: u64 = 8 * 1024 * 1024;

/// One verified slice inside a coalesced range: the executor reads the
/// whole range in one call but still checks BLAKE3 per original chunk, so
/// coalescing never weakens verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPart {
    pub len: u32,
    /// Hex BLAKE3 of this part's raw bytes.
    pub blake3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RebuildOp {
    /// Copy bytes straight from the previous installed artifact.
    CopyPreviousRange {
        source_offset: u64,
        output_offset: u64,
        len: u64,
        /// Per-chunk hashes covering the range end to end, in order.
        parts: Vec<PlanPart>,
    },
    /// Read one chunk from the local content-addressable cache.
    CopyCacheChunk {
        chunk_hash: String,
        output_offset: u64,
        len: u32,
    },
    /// Chunk must come from the origin (it will land in the cache first).
    FetchNetworkChunk {
        chunk_hash: String,
        output_offset: u64,
        len: u32,
    },
}

impl RebuildOp {
    pub fn output_range(&self) -> (u64, u64) {
        match self {
            RebuildOp::CopyPreviousRange {
                output_offset, len, ..
            } => (*output_offset, *len),
            RebuildOp::CopyCacheChunk {
                output_offset, len, ..
            } => (*output_offset, *len as u64),
            RebuildOp::FetchNetworkChunk {
                output_offset, len, ..
            } => (*output_offset, *len as u64),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanStats {
    pub ops_total: u64,
    pub ops_before_coalescing: u64,
    pub coalesced_ops: u64,
    pub copy_previous_range_ops: u64,
    pub copy_cache_chunk_ops: u64,
    pub fetch_chunk_ops: u64,
    pub previous_artifact_bytes: u64,
    pub cache_chunk_bytes: u64,
    pub network_bytes: u64,
    pub source_selection_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconstructionPlan {
    pub asset: String,
    pub output_name: String,
    pub target_size: u64,
    pub ops: Vec<RebuildOp>,
    pub stats: PlanStats,
}

/// One required output chunk, in output order (offsets must be contiguous).
#[derive(Debug, Clone)]
pub struct NeededChunk {
    /// Hex BLAKE3 of the chunk's raw bytes.
    pub hash: String,
    pub len: u32,
    pub output_offset: u64,
}

/// Where a chunk's bytes can already be found locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Present in the chunk cache.
    Cache,
    /// Present in the previous installed artifact at this source offset.
    Previous { source_offset: u64 },
    /// Present in both.
    Both { source_offset: u64 },
    /// Must be fetched.
    Missing,
}

/// Relative source costs, following the v0.6.0 design cost model. Only the
/// *relative* magnitudes matter: network bytes dominate, then seeks, then
/// local reads.
fn score(network_bytes: u64, disk_read_bytes: u64, seeks: u32, verify: u32) -> u64 {
    network_bytes * 100 + disk_read_bytes + seeks as u64 * 4096 + verify as u64 * 512
}

/// Build a deterministic hybrid plan: for each needed chunk pick the
/// cheapest source, preferring (on ties) a previous-artifact range that
/// continues the last one, then any previous range, then the cache.
/// Adjacent previous-range ops are coalesced up to [`MAX_COALESCED_RANGE`].
pub fn plan(
    asset: &str,
    output_name: &str,
    chunks: &[NeededChunk],
    availability: impl Fn(&NeededChunk) -> Availability,
) -> ReconstructionPlan {
    let started = std::time::Instant::now();
    let mut ops: Vec<RebuildOp> = Vec::with_capacity(chunks.len());
    let mut stats = PlanStats::default();
    let mut target_size = 0u64;
    // End of the last chosen previous-range read (source side), for
    // contiguity-aware scoring.
    let mut prev_source_end: Option<u64> = None;

    for c in chunks {
        target_size = target_size.max(c.output_offset + c.len as u64);
        stats.ops_before_coalescing += 1;
        let avail = availability(c);
        let (use_previous, source_offset) = match avail {
            Availability::Missing => {
                stats.fetch_chunk_ops += 1;
                stats.network_bytes += c.len as u64;
                ops.push(RebuildOp::FetchNetworkChunk {
                    chunk_hash: c.hash.clone(),
                    output_offset: c.output_offset,
                    len: c.len,
                });
                prev_source_end = None;
                continue;
            }
            Availability::Cache => (false, 0),
            Availability::Previous { source_offset } => (true, source_offset),
            Availability::Both { source_offset } => {
                let contiguous = prev_source_end == Some(source_offset);
                let prev_score = score(0, c.len as u64, if contiguous { 0 } else { 1 }, 1);
                let cache_score = score(0, c.len as u64, 1, 1);
                // Ties break toward the previous artifact: one open file
                // read sequentially beats many small cache files.
                (prev_score <= cache_score, source_offset)
            }
        };

        if use_previous {
            stats.copy_previous_range_ops += 1;
            stats.previous_artifact_bytes += c.len as u64;
            prev_source_end = Some(source_offset + c.len as u64);
            let part = PlanPart {
                len: c.len,
                blake3: c.hash.clone(),
            };
            // Coalesce with the previous op when source and output are both
            // contiguous and the budget allows.
            if let Some(RebuildOp::CopyPreviousRange {
                source_offset: po,
                output_offset: pn,
                len: pl,
                parts,
            }) = ops.last_mut()
            {
                if *po + *pl == source_offset
                    && *pn + *pl == c.output_offset
                    && *pl + c.len as u64 <= MAX_COALESCED_RANGE
                {
                    *pl += c.len as u64;
                    parts.push(part);
                    continue;
                }
            }
            ops.push(RebuildOp::CopyPreviousRange {
                source_offset,
                output_offset: c.output_offset,
                len: c.len as u64,
                parts: vec![part],
            });
        } else {
            stats.copy_cache_chunk_ops += 1;
            stats.cache_chunk_bytes += c.len as u64;
            prev_source_end = None;
            ops.push(RebuildOp::CopyCacheChunk {
                chunk_hash: c.hash.clone(),
                output_offset: c.output_offset,
                len: c.len,
            });
        }
    }

    stats.ops_total = ops.len() as u64;
    stats.coalesced_ops = stats.ops_before_coalescing - stats.ops_total;
    stats.source_selection_ms = started.elapsed().as_secs_f64() * 1000.0;
    ReconstructionPlan {
        asset: asset.to_string(),
        output_name: output_name.to_string(),
        target_size,
        ops,
        stats,
    }
}

/// Convenience: availability from a cache membership test plus a
/// hash → source-offset map of the previous artifact.
pub fn availability_from_sets<'a>(
    cache_contains: impl Fn(&str) -> bool + 'a,
    previous: &'a HashMap<String, u64>,
) -> impl Fn(&NeededChunk) -> Availability + 'a {
    move |c: &NeededChunk| {
        let in_cache = cache_contains(&c.hash);
        match (in_cache, previous.get(&c.hash)) {
            (true, Some(&off)) => Availability::Both { source_offset: off },
            (false, Some(&off)) => Availability::Previous { source_offset: off },
            (true, None) => Availability::Cache,
            (false, None) => Availability::Missing,
        }
    }
}

/// A plan is valid when its ops tile `0..target_size` exactly, in order,
/// and every coalesced range's parts cover its length.
pub fn validate(plan: &ReconstructionPlan) -> Result<(), String> {
    let mut at = 0u64;
    for op in &plan.ops {
        let (off, len) = op.output_range();
        if off != at {
            return Err(format!(
                "plan gap/overlap at output offset {at} (op starts at {off})"
            ));
        }
        if let RebuildOp::CopyPreviousRange { parts, len, .. } = op {
            let parts_len: u64 = parts.iter().map(|p| p.len as u64).sum();
            if parts_len != *len {
                return Err(format!(
                    "coalesced range parts cover {parts_len} of {len} bytes"
                ));
            }
        }
        at += len;
    }
    if at != plan.target_size {
        return Err(format!(
            "plan covers {at} of {} target bytes",
            plan.target_size
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(lens: &[u32]) -> Vec<NeededChunk> {
        let mut out = Vec::new();
        let mut off = 0u64;
        for (i, &len) in lens.iter().enumerate() {
            out.push(NeededChunk {
                hash: format!("{i:064x}"),
                len,
                output_offset: off,
            });
            off += len as u64;
        }
        out
    }

    #[test]
    fn v05_flow_is_a_special_case() {
        // AC1/AC2: cache + network only expresses the old behaviour.
        let cs = chunks(&[100, 200, 300]);
        let cached: std::collections::HashSet<String> = [cs[0].hash.clone()].into();
        let previous = HashMap::new();
        let p = plan(
            "asset",
            "out.bin",
            &cs,
            availability_from_sets(|h| cached.contains(h), &previous),
        );
        validate(&p).unwrap();
        assert_eq!(p.stats.copy_previous_range_ops, 0);
        assert_eq!(p.stats.copy_cache_chunk_ops, 1);
        assert_eq!(p.stats.fetch_chunk_ops, 2);
        assert_eq!(p.stats.network_bytes, 500);
    }

    #[test]
    fn contiguous_previous_ranges_coalesce() {
        let cs = chunks(&[100, 100, 100, 100]);
        // All four live contiguously in the previous artifact.
        let previous: HashMap<String, u64> = cs
            .iter()
            .map(|c| (c.hash.clone(), c.output_offset + 5000))
            .collect();
        let p = plan(
            "asset",
            "out.bin",
            &cs,
            availability_from_sets(|_| false, &previous),
        );
        validate(&p).unwrap();
        assert_eq!(p.ops.len(), 1, "should coalesce into one range: {p:?}");
        assert_eq!(p.stats.ops_before_coalescing, 4);
        assert_eq!(p.stats.coalesced_ops, 3);
        assert_eq!(p.stats.previous_artifact_bytes, 400);
        match &p.ops[0] {
            RebuildOp::CopyPreviousRange {
                source_offset,
                len,
                parts,
                ..
            } => {
                assert_eq!(*source_offset, 5000);
                assert_eq!(*len, 400);
                assert_eq!(parts.len(), 4);
            }
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn coalescing_respects_max_range() {
        let n = 40;
        let lens: Vec<u32> = std::iter::repeat_n(512 * 1024, n).collect();
        let cs = chunks(&lens);
        let previous: HashMap<String, u64> = cs
            .iter()
            .map(|c| (c.hash.clone(), c.output_offset))
            .collect();
        let p = plan(
            "asset",
            "out.bin",
            &cs,
            availability_from_sets(|_| false, &previous),
        );
        validate(&p).unwrap();
        for op in &p.ops {
            if let RebuildOp::CopyPreviousRange { len, .. } = op {
                assert!(*len <= MAX_COALESCED_RANGE);
            }
        }
        // 40 × 512 KiB = 20 MiB → at least 3 ops under the 8 MiB budget.
        assert!(p.ops.len() >= 3);
    }

    #[test]
    fn previous_beats_cache_when_contiguous() {
        let cs = chunks(&[100, 100]);
        let previous: HashMap<String, u64> = cs
            .iter()
            .map(|c| (c.hash.clone(), c.output_offset))
            .collect();
        // Everything is in the cache too.
        let p = plan(
            "asset",
            "out.bin",
            &cs,
            availability_from_sets(|_| true, &previous),
        );
        validate(&p).unwrap();
        assert_eq!(p.stats.copy_previous_range_ops, 2);
        assert_eq!(p.stats.copy_cache_chunk_ops, 0);
    }

    #[test]
    fn plans_are_deterministic_and_serializable() {
        let cs = chunks(&[64, 64, 64]);
        let previous: HashMap<String, u64> = [(cs[1].hash.clone(), 999u64)].into();
        let avail = availability_from_sets(|h| h == cs[0].hash, &previous);
        let p1 = plan("a", "o", &cs, &avail);
        let p2 = plan("a", "o", &cs, &avail);
        assert_eq!(p1.ops, p2.ops);
        let json = serde_json::to_string_pretty(&p1).unwrap();
        let back: ReconstructionPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ops, p1.ops);
    }

    #[test]
    fn validate_catches_gaps() {
        let cs = chunks(&[100, 100]);
        let previous = HashMap::new();
        let mut p = plan("a", "o", &cs, availability_from_sets(|_| true, &previous));
        validate(&p).unwrap();
        p.ops.remove(0);
        assert!(validate(&p).is_err());
    }
}
