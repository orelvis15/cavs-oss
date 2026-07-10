//! Hybrid reconstruction support (v0.6.0): the previous installed artifact
//! as a first-class byte source.
//!
//! The previous artifact is mapped read-only, chunked with the same profile
//! the packer used for the new version (recorded in the manifest meta), and
//! indexed by chunk hash. Chunks the new version shares with the old one
//! are then copied straight from the old file — verified per range by
//! BLAKE3 before a single byte lands in the output — instead of travelling
//! over the network or through the chunk cache.
//!
//! Safety rule: the previous artifact is never trusted blindly. Every
//! copied range re-hashes to its expected chunk hash at read time, and the
//! whole output still passes the manifest's SHA-256 before promotion. A
//! range that fails verification demotes to cache/network transparently.

use crate::cache::ChunkCache;
use anyhow::{Context, Result};
use cavs_chunker::ChunkMode;
use cavs_hash::to_hex;
use cavs_proto::errors::ErrorCode;
use cavs_proto::Manifest;
use cavs_rebuild_plan::{NeededChunk, RebuildOp, ReconstructionPlan};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Hybrid-related fetch options, threaded from the CLI.
#[derive(Debug, Clone, Default)]
pub struct HybridOpts {
    pub previous_artifact: Option<PathBuf>,
    /// `--no-hybrid` sets this false: planning falls back to v0.5 behaviour
    /// (cache + network only).
    pub enabled: bool,
    pub dump_plan: Option<PathBuf>,
    /// `--force-reconstruct` disables every no-op shortcut.
    pub force_reconstruct: bool,
    /// Directory mode: delete files not present in the new container.
    pub prune: bool,
}

/// Map a packer profile label (manifest meta `profile:<name>`) back to the
/// exact chunking parameters. Must mirror `ChunkProfile::to_mode` in
/// cavs-cli: matching the new version's boundaries is what makes old-file
/// chunks hash-identical to manifest chunks.
pub fn mode_from_profile_label(label: Option<&str>) -> ChunkMode {
    match label {
        Some("fixed-256k") => ChunkMode::Fixed { size: 256 * 1024 },
        Some("fixed-512k") => ChunkMode::Fixed { size: 512 * 1024 },
        Some("fixed-1m") => ChunkMode::Fixed { size: 1024 * 1024 },
        Some("fastcdc-16k") => ChunkMode::Cdc {
            min: 4 * 1024,
            avg: 16 * 1024,
            max: 64 * 1024,
            norm: cavs_chunker::NORM_TIGHT,
        },
        Some("fastcdc-32k") => ChunkMode::Cdc {
            min: 8 * 1024,
            avg: 32 * 1024,
            max: 128 * 1024,
            norm: cavs_chunker::NORM_TIGHT,
        },
        Some("fastcdc-128k") => ChunkMode::Cdc {
            min: 32 * 1024,
            avg: 128 * 1024,
            max: 512 * 1024,
            norm: cavs_chunker::NORM_DEFAULT,
        },
        // 1.4.0: the -n3 labels reuse the same sizes with normalization
        // level 3 — new labels so published n1 streams keep their boundaries.
        Some("fastcdc-64k-n3") => ChunkMode::Cdc {
            min: 16 * 1024,
            avg: 64 * 1024,
            max: 256 * 1024,
            norm: cavs_chunker::NORM_TIGHT,
        },
        Some("fastcdc-128k-n3") => ChunkMode::Cdc {
            min: 32 * 1024,
            avg: 128 * 1024,
            max: 512 * 1024,
            norm: cavs_chunker::NORM_TIGHT,
        },
        Some("fastcdc-256k") => ChunkMode::Cdc {
            min: 64 * 1024,
            avg: 256 * 1024,
            max: 1024 * 1024,
            norm: cavs_chunker::NORM_DEFAULT,
        },
        // fastcdc-64k and the packer default share these parameters.
        _ => ChunkMode::Cdc {
            min: 16 * 1024,
            avg: 64 * 1024,
            max: 256 * 1024,
            norm: cavs_chunker::NORM_DEFAULT,
        },
    }
}

/// The previous installed artifact, mapped read-only and indexed by the
/// chunk hashes the new manifest needs.
pub struct PreviousArtifact {
    map: memmap2::Mmap,
    /// Hex chunk hash → source offset (first occurrence wins; offsets of
    /// equal-hash chunks are interchangeable by definition).
    pub index: HashMap<String, u64>,
    pub indexed_ms: u64,
}

impl PreviousArtifact {
    /// Open and index the previous artifact. Only hashes in `needed` are
    /// kept, so the index stays proportional to the new asset.
    pub fn open_and_index(path: &Path, mode: ChunkMode, needed: &HashSet<String>) -> Result<Self> {
        let started = std::time::Instant::now();
        let file = std::fs::File::open(path).with_context(|| {
            ErrorCode::PreviousArtifactMissing.msg(format!("cannot open {}", path.display()))
        })?;
        // Safety: read-only map; concurrent truncation would at worst fault
        // this process, and the artifact is expected to be quiescent.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        let mut index = HashMap::new();
        for range in cavs_chunker::split(&map, mode) {
            let hash = to_hex(&cavs_hash::hash_chunk(&map[range.clone()]));
            if needed.contains(&hash) {
                index.entry(hash).or_insert(range.start as u64);
            }
        }
        Ok(Self {
            map,
            index,
            indexed_ms: started.elapsed().as_millis() as u64,
        })
    }

    pub fn read_range(&self, offset: u64, len: u64) -> Option<&[u8]> {
        let end = offset.checked_add(len)?;
        self.map.get(offset as usize..end as usize)
    }
}

/// Ordered chunk list of one data track, with output offsets: the plan
/// input. Segments are walked in presentation order, matching how the
/// track is reconstructed.
pub fn needed_chunks_for_track(manifest: &Manifest, track_id: u32) -> Vec<NeededChunk> {
    let mut segs: Vec<_> = manifest
        .segments
        .iter()
        .filter(|s| s.track_id == track_id)
        .collect();
    segs.sort_by_key(|s| (s.pts_start, s.segment_id));
    let mut out = Vec::new();
    let mut offset = 0u64;
    for seg in segs {
        for c in &seg.chunks {
            out.push(NeededChunk {
                hash: c.hash.clone(),
                len: c.len,
                output_offset: offset,
            });
            offset += c.len as u64;
        }
    }
    out
}

/// Per-execution source accounting (feeds the fetch stats).
#[derive(Debug, Default, Clone)]
pub struct ExecOutcome {
    pub previous_artifact_bytes: u64,
    pub cache_chunk_bytes: u64,
    /// Chunks whose previous-artifact range failed BLAKE3 and were demoted
    /// to cache/network.
    pub demoted_chunks: u64,
    /// Bytes fetched directly by hash to repair demoted/missing chunks.
    pub repair_wire_bytes: u64,
}

/// Execute one plan into `write` (an ordered byte sink). Every byte is
/// verified before being written: previous ranges re-hash per part, cache
/// reads verify inside the cache, network repairs verify explicitly.
/// `fetch_chunk` resolves a hex hash to verified raw bytes from the origin.
pub fn execute_plan(
    plan: &ReconstructionPlan,
    prev: Option<&PreviousArtifact>,
    cache: &ChunkCache,
    mut write: impl FnMut(&[u8]) -> Result<()>,
    mut fetch_chunk: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<ExecOutcome> {
    cavs_rebuild_plan::validate(plan)
        .map_err(|e| anyhow::anyhow!(ErrorCode::HybridPlanInvalid.msg(e)))?;
    let mut outcome = ExecOutcome::default();

    // Resolve one chunk through the fallback chain cache → origin.
    let mut chunk_via_fallback = |hash_hex: &str, outcome: &mut ExecOutcome| -> Result<Vec<u8>> {
        let hash =
            cavs_hash::from_hex(hash_hex).with_context(|| format!("bad chunk hash {hash_hex}"))?;
        if let Some(bytes) = cache.get(&hash)? {
            outcome.cache_chunk_bytes += bytes.len() as u64;
            return Ok(bytes);
        }
        let bytes = fetch_chunk(hash_hex)?;
        if cavs_hash::hash_chunk(&bytes) != hash {
            anyhow::bail!(
                "{}",
                ErrorCode::ChunkHashMismatch.msg(format!(
                    "repaired chunk {hash_hex} failed hash verification"
                ))
            );
        }
        outcome.repair_wire_bytes += bytes.len() as u64;
        cache.put(&hash, &bytes)?;
        Ok(bytes)
    };

    for op in &plan.ops {
        match op {
            RebuildOp::CopyPreviousRange {
                source_offset,
                len,
                parts,
                ..
            } => {
                let range = prev.and_then(|p| p.read_range(*source_offset, *len));
                match range {
                    Some(bytes) => {
                        // Verify every part before any byte of the range is
                        // written; a single bad part demotes only itself.
                        let mut at = 0usize;
                        for part in parts {
                            let end = at + part.len as usize;
                            let slice = &bytes[at..end];
                            let expected = cavs_hash::from_hex(&part.blake3)
                                .with_context(|| format!("bad hash {}", part.blake3))?;
                            if cavs_hash::hash_chunk(slice) == expected {
                                outcome.previous_artifact_bytes += slice.len() as u64;
                                write(slice)?;
                            } else {
                                eprintln!(
                                    "[hybrid] {}",
                                    ErrorCode::PreviousArtifactMismatch.msg(format!(
                                        "range at {} failed verification; falling back to cache/network",
                                        *source_offset + at as u64
                                    ))
                                );
                                outcome.demoted_chunks += 1;
                                let good = chunk_via_fallback(&part.blake3, &mut outcome)?;
                                write(&good)?;
                            }
                            at = end;
                        }
                    }
                    None => {
                        // Previous artifact vanished or is too short: demote
                        // the whole range part by part.
                        eprintln!(
                            "[hybrid] {}",
                            ErrorCode::PreviousArtifactMismatch.msg(format!(
                                "range {}..{} unreadable; falling back to cache/network",
                                source_offset,
                                source_offset + len
                            ))
                        );
                        for part in parts {
                            outcome.demoted_chunks += 1;
                            let good = chunk_via_fallback(&part.blake3, &mut outcome)?;
                            write(&good)?;
                        }
                    }
                }
            }
            RebuildOp::CopyCacheChunk { chunk_hash, .. }
            | RebuildOp::FetchNetworkChunk { chunk_hash, .. } => {
                // Network chunks were batched into the cache before
                // execution, so both resolve through the same chain.
                let bytes = chunk_via_fallback(chunk_hash, &mut outcome)?;
                write(&bytes)?;
            }
        }
    }
    Ok(outcome)
}

/// Streaming SHA-256 check of an existing file (no-op detection). Returns
/// false on any error (missing file, unreadable) — never a false positive.
pub fn file_matches_sha256(path: &Path, expected_hex: &str) -> bool {
    use sha2::{Digest, Sha256};
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        use std::io::Read as _;
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    digest.eq_ignore_ascii_case(expected_hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cavs_rebuild_plan::{availability_from_sets, plan};

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    /// The label map must mirror `ChunkProfile::to_mode` in cavs-cli —
    /// including the tight-normalization 16k/32k profiles (1.3.0). A
    /// mismatch would silently kill hybrid reuse for those streams.
    #[test]
    fn new_small_profile_labels_map_to_tight_modes() {
        assert_eq!(
            mode_from_profile_label(Some("fastcdc-16k")),
            ChunkMode::Cdc {
                min: 4 * 1024,
                avg: 16 * 1024,
                max: 64 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            }
        );
        assert_eq!(
            mode_from_profile_label(Some("fastcdc-32k")),
            ChunkMode::Cdc {
                min: 8 * 1024,
                avg: 32 * 1024,
                max: 128 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            }
        );
        // Unknown labels keep falling back to the 64k default.
        assert_eq!(
            mode_from_profile_label(None),
            mode_from_profile_label(Some("fastcdc-64k"))
        );
    }

    /// The 1.4.0 -n3 labels reuse the n1 sizes with tight normalization. A
    /// client that maps them to the n1 mode would re-chunk the previous
    /// artifact on the wrong boundaries and lose all hybrid reuse.
    #[test]
    fn n3_profile_labels_map_to_tight_modes() {
        assert_eq!(
            mode_from_profile_label(Some("fastcdc-64k-n3")),
            ChunkMode::Cdc {
                min: 16 * 1024,
                avg: 64 * 1024,
                max: 256 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            }
        );
        assert_eq!(
            mode_from_profile_label(Some("fastcdc-128k-n3")),
            ChunkMode::Cdc {
                min: 32 * 1024,
                avg: 128 * 1024,
                max: 512 * 1024,
                norm: cavs_chunker::NORM_TIGHT,
            }
        );
        // The -n3 label must NOT collapse to its n1 sibling's boundaries.
        assert_ne!(
            mode_from_profile_label(Some("fastcdc-64k-n3")),
            mode_from_profile_label(Some("fastcdc-64k"))
        );
    }

    /// Chunk `data`, index a previous artifact, plan and execute — the
    /// whole hybrid path minus the HTTP layer.
    #[test]
    fn plan_and_execute_from_previous_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let mode = ChunkMode::Cdc {
            min: 16 * 1024,
            avg: 64 * 1024,
            max: 256 * 1024,
            norm: cavs_chunker::NORM_DEFAULT,
        };

        // v1 on disk; v2 = v1 with a rewritten slice in the middle.
        let v1 = pseudo_random(2 * 1024 * 1024, 40);
        let mut v2 = v1.clone();
        v2[1_000_000..1_050_000].copy_from_slice(&pseudo_random(50_000, 41));
        let prev_path = dir.path().join("v1.pck");
        std::fs::write(&prev_path, &v1).unwrap();

        // The "manifest": v2's chunk plan.
        let mut needed = Vec::new();
        let mut offset = 0u64;
        for range in cavs_chunker::split(&v2, mode) {
            let bytes = &v2[range.clone()];
            needed.push(NeededChunk {
                hash: to_hex(&cavs_hash::hash_chunk(bytes)),
                len: bytes.len() as u32,
                output_offset: offset,
            });
            offset += bytes.len() as u64;
        }
        let needed_set: HashSet<String> = needed.iter().map(|c| c.hash.clone()).collect();

        let prev = PreviousArtifact::open_and_index(&prev_path, mode, &needed_set).unwrap();
        assert!(!prev.index.is_empty(), "previous artifact matched nothing");

        let cache = ChunkCache::open(&dir.path().join("cache")).unwrap();
        let p = plan(
            "v2",
            "v2.pck",
            &needed,
            availability_from_sets(|_| false, &prev.index),
        );
        assert!(p.stats.previous_artifact_bytes > v2.len() as u64 / 2);

        // "Network" = the chunks of v2 itself, verified by hash.
        let by_hash: HashMap<String, Vec<u8>> = needed
            .iter()
            .map(|c| {
                let s = c.output_offset as usize;
                (c.hash.clone(), v2[s..s + c.len as usize].to_vec())
            })
            .collect();
        let mut out = Vec::new();
        let outcome = execute_plan(
            &p,
            Some(&prev),
            &cache,
            |bytes| {
                out.extend_from_slice(bytes);
                Ok(())
            },
            |hash| Ok(by_hash[hash].clone()),
        )
        .unwrap();
        assert_eq!(out, v2, "hybrid output must be byte-identical");
        assert_eq!(outcome.demoted_chunks, 0);
        assert!(outcome.previous_artifact_bytes > 0);
    }

    /// A corrupt previous artifact must demote to the fallback chain and
    /// still produce byte-identical output.
    #[test]
    fn corrupt_previous_artifact_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let mode = ChunkMode::Fixed { size: 64 * 1024 };

        let v1 = pseudo_random(512 * 1024, 50);
        // Corrupt the on-disk previous copy AFTER computing the index from
        // the pristine bytes (simulates decay between indexing and reading:
        // the per-range verification is what catches it).
        let prev_path = dir.path().join("v1.bin");
        std::fs::write(&prev_path, &v1).unwrap();

        let mut needed = Vec::new();
        let mut offset = 0u64;
        for range in cavs_chunker::split(&v1, mode) {
            let bytes = &v1[range.clone()];
            needed.push(NeededChunk {
                hash: to_hex(&cavs_hash::hash_chunk(bytes)),
                len: bytes.len() as u32,
                output_offset: offset,
            });
            offset += bytes.len() as u64;
        }
        let needed_set: HashSet<String> = needed.iter().map(|c| c.hash.clone()).collect();
        let prev = PreviousArtifact::open_and_index(&prev_path, mode, &needed_set).unwrap();

        // Now corrupt the file under the map's feet by rewriting it.
        let mut tampered = v1.clone();
        tampered[100_000] ^= 0xff;
        std::fs::write(&prev_path, &tampered).unwrap();
        let prev_tampered =
            PreviousArtifact::open_and_index(&prev_path, mode, &HashSet::new()).unwrap();
        // Re-borrow the pristine index against the tampered bytes.
        let prev = PreviousArtifact {
            map: prev_tampered.map,
            index: prev.index,
            indexed_ms: 0,
        };

        let cache = ChunkCache::open(&dir.path().join("cache")).unwrap();
        let p = plan(
            "v1",
            "v1.bin",
            &needed,
            availability_from_sets(|_| false, &prev.index),
        );
        let by_hash: HashMap<String, Vec<u8>> = needed
            .iter()
            .map(|c| {
                let s = c.output_offset as usize;
                (c.hash.clone(), v1[s..s + c.len as usize].to_vec())
            })
            .collect();
        let mut out = Vec::new();
        let outcome = execute_plan(
            &p,
            Some(&prev),
            &cache,
            |bytes| {
                out.extend_from_slice(bytes);
                Ok(())
            },
            |hash| Ok(by_hash[hash].clone()),
        )
        .unwrap();
        assert_eq!(out, v1, "fallback output must be byte-identical");
        assert_eq!(
            outcome.demoted_chunks, 1,
            "exactly the corrupt chunk demotes"
        );
        assert!(outcome.repair_wire_bytes > 0);
    }

    #[test]
    fn profile_labels_map_to_pack_parameters() {
        assert_eq!(
            mode_from_profile_label(Some("fixed-256k")),
            ChunkMode::Fixed { size: 256 * 1024 }
        );
        assert_eq!(
            mode_from_profile_label(Some("fastcdc-64k")),
            mode_from_profile_label(None)
        );
        assert_eq!(
            mode_from_profile_label(Some("fastcdc-128k")),
            ChunkMode::Cdc {
                min: 32 * 1024,
                avg: 128 * 1024,
                max: 512 * 1024,
                norm: cavs_chunker::NORM_DEFAULT,
            }
        );
    }
}
