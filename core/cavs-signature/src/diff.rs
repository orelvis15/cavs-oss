//! Hybrid diff: find ranges of a *new* payload that already exist in an
//! *old* source described only by a [`CavsSignature`].
//!
//! rsync-style scan: a weak 32-bit rolling hash slides over the new bytes
//! and is looked up in an index of the old blocks; candidates are confirmed
//! with BLAKE3 before a single byte is reused. Unmatched bytes become
//! inline data, capped at [`MAX_INLINE_DATA`] per op so plans stay
//! streaming-friendly.
//!
//! The result is source-agnostic: `CopyOldRange` says "these new-file bytes
//! equal old entry X at offset Y", which a client maps to a previous
//! installed artifact and the delta benchmark maps to a copy operation.

use crate::weak::RollingWeak;
use crate::{CavsSignature, EntryKind, SignatureBlockHash};
use cavs_hash::hash_chunk;
use std::collections::HashMap;

/// Inline (fresh) data is capped at 4 MB per op, so a run of unmatched
/// bytes is split into streaming-friendly messages rather than one huge one.
pub const MAX_INLINE_DATA: u64 = 4 * 1024 * 1024;

/// One reconstruction instruction for the new payload, in output order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    /// Bytes `new_offset .. new_offset+len` of the new payload equal bytes
    /// `old_offset .. old_offset+len` of old entry `entry_id`.
    CopyOldRange {
        entry_id: u32,
        old_offset: u64,
        new_offset: u64,
        len: u64,
    },
    /// Fresh bytes that must travel in the patch/wire.
    InlineData { new_offset: u64, len: u64 },
}

#[derive(Debug, Clone, Default)]
pub struct DiffPlan {
    pub ops: Vec<DiffOp>,
    /// Bytes served from the old source.
    pub reused_bytes: u64,
    /// Bytes that must be shipped as fresh data.
    pub inline_bytes: u64,
    /// Copy ops emitted before adjacent ranges were merged.
    pub ops_before_coalescing: u64,
}

/// Weak-hash prefilter index over an old signature's full-size blocks.
/// (Short tail blocks can't be found by a fixed-size rolling window; they
/// are cheap to re-send as inline data.)
pub struct WeakHashIndex<'a> {
    sig: &'a CavsSignature,
    by_weak: HashMap<u32, Vec<u32>>,
    /// entry_id -> path, for preferred-source scoring.
    entry_path: HashMap<u32, &'a str>,
}

impl<'a> WeakHashIndex<'a> {
    pub fn build(sig: &'a CavsSignature) -> Self {
        let mut by_weak: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, b) in sig.blocks.iter().enumerate() {
            if b.len == sig.block_size {
                by_weak.entry(b.weak32).or_default().push(i as u32);
            }
        }
        let entry_path = sig
            .entries
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| (e.entry_id, e.path.as_str()))
            .collect();
        Self {
            sig,
            by_weak,
            entry_path,
        }
    }

    pub fn block(&self, idx: u32) -> &SignatureBlockHash {
        &self.sig.blocks[idx as usize]
    }

    /// Candidate blocks for a weak hash (still unconfirmed).
    fn candidates(&self, weak: u32) -> &[u32] {
        self.by_weak.get(&weak).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Preferred source selection among candidates whose strong hash
    /// matches: continue the previous copy if possible, then prefer the
    /// old entry whose path equals the target path, then the lowest offset
    /// (deterministic).
    fn pick(
        &self,
        matching: &[u32],
        target_path: Option<&str>,
        prev: Option<(u32, u64)>,
    ) -> Option<u32> {
        matching.iter().copied().max_by_key(|&i| {
            let b = self.block(i);
            let mut score: i64 = 0;
            if let Some((prev_entry, prev_end)) = prev {
                if b.entry_id == prev_entry && b.offset == prev_end {
                    score += 500;
                }
            }
            if let (Some(target), Some(&path)) = (target_path, self.entry_path.get(&b.entry_id)) {
                if path == target {
                    score += 1000;
                }
            }
            // Deterministic tie-break: earliest offset wins.
            score * 1_000_000 - (b.offset.min(500_000) as i64) - (b.entry_id as i64)
        })
    }
}

/// Diff `new` against the old signature. `target_path` enables same-path
/// preference in directory mode (pass the new file's relative path).
pub fn diff_bytes(index: &WeakHashIndex, new: &[u8], target_path: Option<&str>) -> DiffPlan {
    let bs = index.sig.block_size as usize;
    let mut plan = DiffPlan::default();
    let mut raw_ops: Vec<DiffOp> = Vec::new();
    let mut owed_start = 0u64;
    let mut prev_copy: Option<(u32, u64)> = None;

    let mut pos = 0usize;
    if new.len() >= bs && !index.by_weak.is_empty() {
        let mut rw = RollingWeak::new(&new[0..bs]);
        loop {
            let weak = rw.digest();
            let mut matched = None;
            let cands = index.candidates(weak);
            if !cands.is_empty() {
                let strong = hash_chunk(&new[pos..pos + bs]);
                let matching: Vec<u32> = cands
                    .iter()
                    .copied()
                    .filter(|&i| index.block(i).strong_blake3 == strong)
                    .collect();
                matched = index.pick(&matching, target_path, prev_copy);
            }
            if let Some(bi) = matched {
                let b = index.block(bi);
                emit_inline(&mut raw_ops, &mut plan, owed_start, pos as u64);
                raw_ops.push(DiffOp::CopyOldRange {
                    entry_id: b.entry_id,
                    old_offset: b.offset,
                    new_offset: pos as u64,
                    len: bs as u64,
                });
                plan.reused_bytes += bs as u64;
                plan.ops_before_coalescing += 1;
                prev_copy = Some((b.entry_id, b.offset + bs as u64));
                pos += bs;
                owed_start = pos as u64;
                if pos + bs > new.len() {
                    break;
                }
                rw = RollingWeak::new(&new[pos..pos + bs]);
            } else {
                if pos + bs >= new.len() {
                    break;
                }
                rw.roll(new[pos], new[pos + bs]);
                pos += 1;
            }
        }
    }
    emit_inline(&mut raw_ops, &mut plan, owed_start, new.len() as u64);

    plan.ops = coalesce(raw_ops);
    plan
}

fn emit_inline(ops: &mut Vec<DiffOp>, plan: &mut DiffPlan, from: u64, to: u64) {
    let mut at = from;
    while at < to {
        let len = (to - at).min(MAX_INLINE_DATA);
        ops.push(DiffOp::InlineData {
            new_offset: at,
            len,
        });
        plan.inline_bytes += len;
        plan.ops_before_coalescing += 1;
        at += len;
    }
}

/// Merge copy ops that are adjacent in both the old entry and the new
/// output (adjacent copy-range coalescing).
fn coalesce(ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let mut out: Vec<DiffOp> = Vec::with_capacity(ops.len());
    for op in ops {
        if let (
            Some(DiffOp::CopyOldRange {
                entry_id: pe,
                old_offset: po,
                new_offset: pn,
                len: pl,
            }),
            DiffOp::CopyOldRange {
                entry_id,
                old_offset,
                new_offset,
                len,
            },
        ) = (out.last_mut(), &op)
        {
            if *pe == *entry_id && *po + *pl == *old_offset && *pn + *pl == *new_offset {
                *pl += *len;
                continue;
            }
        }
        out.push(op);
    }
    out
}

/// Sanity check: ops must tile the new payload exactly, in order, with no
/// gaps or overlaps. Used by tests and by plan consumers before executing.
pub fn validate_coverage(plan: &DiffPlan, new_len: u64) -> bool {
    let mut at = 0u64;
    for op in &plan.ops {
        let (off, len) = match op {
            DiffOp::CopyOldRange {
                new_offset, len, ..
            } => (*new_offset, *len),
            DiffOp::InlineData { new_offset, len } => (*new_offset, *len),
        };
        if off != at {
            return false;
        }
        at += len;
    }
    at == new_len
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SignatureBuilder, SignatureKind};

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    fn sig_of(name: &str, data: &[u8], block: u32) -> CavsSignature {
        let mut b = SignatureBuilder::new(SignatureKind::SingleArtifact, block, "fixed");
        b.begin_entry(name, false, None);
        b.append(data);
        b.finish(name)
    }

    /// Rebuild the new payload from a plan: copies read the old bytes,
    /// inline ops read the new bytes (standing in for patch DATA).
    fn apply(plan: &DiffPlan, old: &[u8], new: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; new.len()];
        for op in &plan.ops {
            match *op {
                DiffOp::CopyOldRange {
                    old_offset,
                    new_offset,
                    len,
                    ..
                } => out[new_offset as usize..(new_offset + len) as usize]
                    .copy_from_slice(&old[old_offset as usize..(old_offset + len) as usize]),
                DiffOp::InlineData { new_offset, len } => out
                    [new_offset as usize..(new_offset + len) as usize]
                    .copy_from_slice(&new[new_offset as usize..(new_offset + len) as usize]),
            }
        }
        out
    }

    #[test]
    fn identical_input_is_all_reuse() {
        let old = pseudo_random(512 * 1024, 1);
        let sig = sig_of("a", &old, 64 * 1024);
        let idx = WeakHashIndex::build(&sig);
        let plan = diff_bytes(&idx, &old, None);
        assert!(validate_coverage(&plan, old.len() as u64));
        assert_eq!(plan.inline_bytes, 0);
        assert_eq!(plan.reused_bytes, old.len() as u64);
        // Fully contiguous reuse coalesces to a single op.
        assert_eq!(plan.ops.len(), 1);
        assert_eq!(apply(&plan, &old, &old), old);
    }

    #[test]
    fn shifted_input_is_found() {
        // AC1: reuse survives an unaligned insertion at the front.
        let old = pseudo_random(512 * 1024, 2);
        let mut new = pseudo_random(1337, 3); // arbitrary unaligned prefix
        new.extend_from_slice(&old);
        let sig = sig_of("a", &old, 64 * 1024);
        let idx = WeakHashIndex::build(&sig);
        let plan = diff_bytes(&idx, &new, None);
        assert!(validate_coverage(&plan, new.len() as u64));
        assert!(
            plan.reused_bytes >= 7 * 64 * 1024,
            "too little reuse: {} bytes",
            plan.reused_bytes
        );
        assert!(plan.inline_bytes < 70 * 1024);
        assert_eq!(apply(&plan, &old, &new), new);
    }

    #[test]
    fn middle_edit_ships_only_the_edit_region() {
        let old = pseudo_random(1024 * 1024, 4);
        let mut new = old.clone();
        new[500_000..500_100].copy_from_slice(&pseudo_random(100, 5));
        let sig = sig_of("a", &old, 64 * 1024);
        let idx = WeakHashIndex::build(&sig);
        let plan = diff_bytes(&idx, &new, None);
        assert!(validate_coverage(&plan, new.len() as u64));
        // The edit dirties at most two blocks worth of data.
        assert!(
            plan.inline_bytes <= 3 * 64 * 1024,
            "inline too large: {}",
            plan.inline_bytes
        );
        assert_eq!(apply(&plan, &old, &new), new);
    }

    #[test]
    fn unrelated_input_is_all_inline_and_capped() {
        let old = pseudo_random(256 * 1024, 6);
        let new = pseudo_random(9 * 1024 * 1024, 7);
        let sig = sig_of("a", &old, 64 * 1024);
        let idx = WeakHashIndex::build(&sig);
        let plan = diff_bytes(&idx, &new, None);
        assert!(validate_coverage(&plan, new.len() as u64));
        assert_eq!(plan.reused_bytes, 0);
        assert_eq!(plan.inline_bytes, new.len() as u64);
        // AC3: inline ops are capped at MAX_INLINE_DATA.
        for op in &plan.ops {
            if let DiffOp::InlineData { len, .. } = op {
                assert!(*len <= MAX_INLINE_DATA);
            }
        }
    }

    #[test]
    fn results_are_deterministic() {
        let old = pseudo_random(300 * 1024, 8);
        let mut new = old.clone();
        new.truncate(250 * 1024);
        new.extend_from_slice(&pseudo_random(80 * 1024, 9));
        let sig = sig_of("a", &old, 64 * 1024);
        let idx = WeakHashIndex::build(&sig);
        let p1 = diff_bytes(&idx, &new, None);
        let p2 = diff_bytes(&idx, &new, None);
        assert_eq!(p1.ops, p2.ops);
    }

    #[test]
    fn duplicate_blocks_prefer_same_path() {
        // Two old files with identical content; the target path must win.
        let content = pseudo_random(128 * 1024, 10);
        let mut b = SignatureBuilder::new(SignatureKind::DirectoryContainer, 64 * 1024, "fixed");
        b.begin_entry("backup/level1.dat", false, None);
        b.append(&content);
        b.begin_entry("levels/level1.dat", false, None);
        b.append(&content);
        let sig = b.finish("build");
        let idx = WeakHashIndex::build(&sig);
        let plan = diff_bytes(&idx, &content, Some("levels/level1.dat"));
        for op in &plan.ops {
            if let DiffOp::CopyOldRange { entry_id, .. } = op {
                assert_eq!(*entry_id, 1, "same-path source must be preferred");
            }
        }
        assert_eq!(plan.reused_bytes, content.len() as u64);
    }
}
