//! Chunk-profile auto-sweep (CAVS v2, P0-3).
//!
//! One chunk size is never optimal for every payload: small CDC chunks win
//! updates but bloat the manifest; large fixed chunks win cold installs of
//! already-compressed content. The encoder therefore *measures* candidate
//! profiles on the actual payload — chunk boundaries, hashes, a sampled
//! compression probe, and (when a previous version is given) real reuse —
//! and picks the cheapest by a weighted cost function.

use anyhow::{bail, Result};
use cavs_chunker::ChunkMode;
use cavs_hash::{hash_chunk, ChunkHash};
use std::collections::HashSet;
use std::time::Instant;

/// Candidate chunking profiles for the sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkProfile {
    Fixed256K,
    Fixed512K,
    Fixed1M,
    FastCdc64K,
    FastCdc128K,
    FastCdc256K,
}

impl ChunkProfile {
    pub const ALL: [ChunkProfile; 6] = [
        ChunkProfile::Fixed256K,
        ChunkProfile::Fixed512K,
        ChunkProfile::Fixed1M,
        ChunkProfile::FastCdc64K,
        ChunkProfile::FastCdc128K,
        ChunkProfile::FastCdc256K,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            ChunkProfile::Fixed256K => "fixed-256k",
            ChunkProfile::Fixed512K => "fixed-512k",
            ChunkProfile::Fixed1M => "fixed-1m",
            ChunkProfile::FastCdc64K => "fastcdc-64k",
            ChunkProfile::FastCdc128K => "fastcdc-128k",
            ChunkProfile::FastCdc256K => "fastcdc-256k",
        }
    }

    pub fn parse(s: &str) -> Result<ChunkProfile> {
        for p in ChunkProfile::ALL {
            if p.label() == s {
                return Ok(p);
            }
        }
        bail!(
            "unknown profile '{s}' (expected one of: {})",
            ChunkProfile::ALL.map(|p| p.label()).join(", ")
        );
    }

    pub fn to_mode(self) -> ChunkMode {
        match self {
            ChunkProfile::Fixed256K => ChunkMode::Fixed { size: 256 * 1024 },
            ChunkProfile::Fixed512K => ChunkMode::Fixed { size: 512 * 1024 },
            ChunkProfile::Fixed1M => ChunkMode::Fixed { size: 1024 * 1024 },
            ChunkProfile::FastCdc64K => ChunkMode::Cdc {
                min: 16 * 1024,
                avg: 64 * 1024,
                max: 256 * 1024,
            },
            ChunkProfile::FastCdc128K => ChunkMode::Cdc {
                min: 32 * 1024,
                avg: 128 * 1024,
                max: 512 * 1024,
            },
            ChunkProfile::FastCdc256K => ChunkMode::Cdc {
                min: 64 * 1024,
                avg: 256 * 1024,
                max: 1024 * 1024,
            },
        }
    }
}

/// Relative weights of each cost dimension (they need not sum to 1).
#[derive(Debug, Clone, Copy)]
pub struct CostWeights {
    pub cold_egress: f64,
    pub update_egress: f64,
    pub storage: f64,
    pub manifest: f64,
    pub request_count: f64,
    pub encode_cpu: f64,
}

impl CostWeights {
    /// Default for games / live updates: update egress dominates.
    pub fn live_updates() -> Self {
        CostWeights {
            cold_egress: 0.20,
            update_egress: 0.40,
            storage: 0.15,
            manifest: 0.10,
            request_count: 0.10,
            encode_cpu: 0.05,
        }
    }

    /// Default for first install / static distribution: cold egress dominates.
    pub fn cold_install() -> Self {
        CostWeights {
            cold_egress: 0.45,
            update_egress: 0.15,
            storage: 0.15,
            manifest: 0.10,
            request_count: 0.10,
            encode_cpu: 0.05,
        }
    }
}

/// Measured/estimated cost of packing a payload with one profile.
#[derive(Debug, Clone)]
pub struct ProfileEstimate {
    pub profile: ChunkProfile,
    pub chunk_count: u64,
    /// Estimated bytes at rest (after per-chunk zstd, sampled).
    pub storage_bytes: u64,
    /// Estimated wire bytes for a cache-less client (storage + manifest).
    pub cold_egress_bytes: u64,
    /// Estimated wire bytes for a client holding `prev` (new chunks only).
    /// Equal to `cold_egress_bytes` when no previous version was given.
    pub update_egress_bytes: u64,
    /// Estimated manifest weight for this chunk count.
    pub manifest_bytes: u64,
    pub request_count: u32,
    pub encode_ms: u64,
    /// Raw bytes reused from `prev` / total raw bytes (0.0 without prev).
    pub reuse_ratio: f64,
}

/// Approximate manifest cost per unique chunk. Each chunk appears in the
/// JSON manifest as a segment entry (hex hash + len + syntax) and again in
/// the signed chunk_table (hex hash + syntax); measured on real manifests.
const MANIFEST_BYTES_PER_CHUNK: u64 = 150;

/// Per-instruction wire overhead of the CVSP batch encoding (tag + hash +
/// compression + lengths).
const WIRE_OVERHEAD_PER_CHUNK: u64 = 42;

/// Cap on how many raw bytes the compression probe compresses per profile.
const COMPRESS_SAMPLE_BUDGET: usize = 32 * 1024 * 1024;

/// The previous version to measure chunk reuse against.
pub enum PrevVersion {
    /// Raw bytes of the previous build: re-chunked with each candidate
    /// profile (models repacking the whole series with the new profile).
    Raw(Vec<u8>),
    /// The chunk hashes of the previously *published* `.cavs`: reuse is
    /// measured against reality, so only profiles whose boundaries line up
    /// with the served version score any reuse. This keeps profile choice
    /// consistent across a version stream.
    ChunkSet(HashSet<ChunkHash>),
}

/// Estimate the cost of packing `data` with `profile`. When `prev` is given,
/// `update_egress_bytes` reflects only the chunks absent from the previous
/// version.
pub fn estimate(
    data: &[u8],
    prev: Option<&PrevVersion>,
    profile: ChunkProfile,
    zstd_level: i32,
) -> ProfileEstimate {
    let started = Instant::now();
    let mode = profile.to_mode();
    let ranges = cavs_chunker::split(data, mode);
    let chunk_count = ranges.len() as u64;

    // Compression probe: compress every k-th chunk within the budget and
    // extrapolate the stored/raw ratio. Mirrors the writer's keep-only-if-
    // it-saves rule (a chunk that gains <1/16 is stored raw).
    let step = {
        let total: usize = data.len();
        (total / COMPRESS_SAMPLE_BUDGET).max(1)
    };
    let mut sampled_raw = 0u64;
    let mut sampled_stored = 0u64;
    for range in ranges.iter().step_by(step) {
        let raw = &data[range.clone()];
        sampled_raw += raw.len() as u64;
        let stored = match zstd::bulk::compress(raw, zstd_level) {
            Ok(c) if (c.len() as u64) < raw.len() as u64 - raw.len() as u64 / 16 => c.len() as u64,
            _ => raw.len() as u64,
        };
        sampled_stored += stored;
    }
    let ratio = if sampled_raw == 0 {
        1.0
    } else {
        sampled_stored as f64 / sampled_raw as f64
    };
    let storage_bytes = (data.len() as f64 * ratio) as u64;

    // Reuse against the previous version: hash-set membership.
    let (new_raw, reuse_ratio) = match prev {
        Some(prev_version) => {
            let owned_hashes;
            let prev_hashes: &HashSet<ChunkHash> = match prev_version {
                PrevVersion::Raw(prev_data) => {
                    owned_hashes = cavs_chunker::split(prev_data, mode)
                        .into_iter()
                        .map(|r| hash_chunk(&prev_data[r]))
                        .collect();
                    &owned_hashes
                }
                PrevVersion::ChunkSet(set) => set,
            };
            let mut new_raw = 0u64;
            for range in &ranges {
                if !prev_hashes.contains(&hash_chunk(&data[range.clone()])) {
                    new_raw += range.len() as u64;
                }
            }
            let total = data.len().max(1) as u64;
            (new_raw, (total - new_raw) as f64 / total as f64)
        }
        None => (data.len() as u64, 0.0),
    };

    let manifest_bytes = chunk_count * MANIFEST_BYTES_PER_CHUNK;
    let wire_overhead = chunk_count * WIRE_OVERHEAD_PER_CHUNK;
    let cold_egress_bytes = storage_bytes + manifest_bytes + wire_overhead;
    let update_egress_bytes = if prev.is_some() {
        (new_raw as f64 * ratio) as u64 + manifest_bytes + wire_overhead
    } else {
        cold_egress_bytes
    };

    // Raw-mode fetches are 3 round-trips (manifest, session, batch) plus a
    // batch per 64 segments; a raw file is a single segment, so requests are
    // effectively constant across profiles. Kept for the cost formula.
    let request_count = 3u32;

    ProfileEstimate {
        profile,
        chunk_count,
        storage_bytes,
        cold_egress_bytes,
        update_egress_bytes,
        manifest_bytes,
        request_count,
        encode_ms: started.elapsed().as_millis() as u64,
        reuse_ratio,
    }
}

/// Load a `--prev` argument: a `.cavs` file (by magic) contributes its real
/// published chunk hashes; anything else is treated as the raw previous
/// build and re-chunked per candidate profile.
pub fn load_prev(path: &std::path::Path) -> Result<PrevVersion> {
    use anyhow::Context as _;
    let mut magic = [0u8; 4];
    {
        use std::io::Read as _;
        let mut f = std::fs::File::open(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let _ = f.read(&mut magic);
    }
    if &magic == b"CAVS" {
        let reader = cavs_format::Reader::open(path)
            .with_context(|| format!("cannot open {} as .cavs", path.display()))?;
        let set: HashSet<ChunkHash> = reader.chunks().iter().map(|c| c.hash).collect();
        return Ok(PrevVersion::ChunkSet(set));
    }
    let data =
        std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(PrevVersion::Raw(data))
}

/// Weighted score: lower is cheaper.
pub fn score(e: &ProfileEstimate, w: &CostWeights) -> f64 {
    e.cold_egress_bytes as f64 * w.cold_egress
        + e.update_egress_bytes as f64 * w.update_egress
        + e.storage_bytes as f64 * w.storage
        + e.manifest_bytes as f64 * w.manifest
        + e.request_count as f64 * 64_000.0 * w.request_count
        + e.encode_ms as f64 * 1024.0 * w.encode_cpu
}

/// Pick the cheapest candidate under `weights`.
pub fn choose_best(candidates: &[ProfileEstimate], weights: &CostWeights) -> ChunkProfile {
    candidates
        .iter()
        .min_by(|a, b| score(a, weights).total_cmp(&score(b, weights)))
        .expect("at least one candidate profile")
        .profile
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
    fn profile_labels_roundtrip() {
        for p in ChunkProfile::ALL {
            assert_eq!(ChunkProfile::parse(p.label()).unwrap(), p);
        }
        assert!(ChunkProfile::parse("fixed-2m").is_err());
    }

    #[test]
    fn incompressible_payload_prefers_large_chunks_for_cold_install() {
        let data = pseudo_random(8 * 1024 * 1024, 11);
        let estimates: Vec<ProfileEstimate> = ChunkProfile::ALL
            .iter()
            .map(|&p| estimate(&data, None, p, 3))
            .collect();
        let best = choose_best(&estimates, &CostWeights::cold_install());
        // Random bytes: storage is ~identical across profiles, so the
        // manifest term must push the choice to the largest chunks.
        assert_eq!(best, ChunkProfile::Fixed1M, "estimates: {estimates:#?}");
    }

    #[test]
    fn shifted_payload_prefers_cdc_for_updates() {
        // v2 = 64 bytes inserted at the front of v1: fixed chunking loses
        // every boundary, CDC re-finds them.
        let v1 = pseudo_random(8 * 1024 * 1024, 42);
        let mut v2 = pseudo_random(64, 999);
        v2.extend_from_slice(&v1);

        let estimates: Vec<ProfileEstimate> = ChunkProfile::ALL
            .iter()
            .map(|&p| estimate(&v2, Some(&PrevVersion::Raw(v1.clone())), p, 3))
            .collect();
        let best = choose_best(&estimates, &CostWeights::live_updates());
        assert!(
            matches!(
                best,
                ChunkProfile::FastCdc64K | ChunkProfile::FastCdc128K | ChunkProfile::FastCdc256K
            ),
            "expected a CDC profile, got {best:?}: {estimates:#?}"
        );
        // And the CDC reuse must be dramatically better than fixed reuse.
        let cdc = estimates
            .iter()
            .find(|e| e.profile == ChunkProfile::FastCdc64K)
            .unwrap();
        let fixed = estimates
            .iter()
            .find(|e| e.profile == ChunkProfile::Fixed256K)
            .unwrap();
        assert!(cdc.reuse_ratio > 0.9, "cdc reuse {}", cdc.reuse_ratio);
        assert!(fixed.reuse_ratio < 0.1, "fixed reuse {}", fixed.reuse_ratio);
    }

    #[test]
    fn identical_versions_have_full_reuse() {
        let v = pseudo_random(2 * 1024 * 1024, 5);
        let e = estimate(&v, Some(&PrevVersion::Raw(v.clone())), ChunkProfile::FastCdc64K, 3);
        assert!(e.reuse_ratio > 0.999);
        assert!(e.update_egress_bytes < e.cold_egress_bytes / 10);
    }
}
