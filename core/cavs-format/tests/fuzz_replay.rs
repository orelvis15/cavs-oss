//! Deterministic mini-fuzz of the `.cavs` container reader (v0.5.0).
//!
//! Same invariants as the coverage-guided targets in `fuzz/`: opening and
//! fully verifying arbitrary or mutated bytes must never panic and never
//! allocate unbounded memory — only succeed or return a `FormatError`.

use cavs_format::{SegmentRecord, TrackKind, TrackRecord, Writer, SEGMENT_FLAG_RANDOM_ACCESS};
use std::path::Path;

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

/// A small but complete container: two tracks' worth of chunks, dedup,
/// compressible and incompressible payloads.
fn build_sample(path: &Path) -> Vec<u8> {
    let mut state = 0xC0FFEEu64;
    let mut random = vec![0u8; 200_000];
    for b in random.iter_mut() {
        *b = xorshift(&mut state) as u8;
    }
    let compressible = vec![0x42u8; 100_000];

    let mut w = Writer::create(path, [9u8; 16], 1000, true).unwrap();
    let mut chunks = Vec::new();
    for data in [&random, &compressible] {
        for part in data.chunks(64 * 1024) {
            chunks.push(w.add_chunk(part).unwrap());
        }
    }
    w.add_track(TrackRecord {
        track_id: 1,
        kind: TrackKind::Data,
        flags: 0,
        codec: "raw".into(),
        name: "build.bin".into(),
        timescale: 1000,
        init_chunks: Vec::new(),
    })
    .unwrap();
    w.add_segment(SegmentRecord {
        segment_id: 0,
        track_id: 1,
        pts_start: 0,
        duration: 0,
        flags: SEGMENT_FLAG_RANDOM_ACCESS,
        chunks,
    })
    .unwrap();
    w.finish().unwrap();
    std::fs::read(path).unwrap()
}

fn open_and_verify(bytes: &[u8], path: &Path) -> bool {
    std::fs::write(path, bytes).unwrap();
    match cavs_format::Reader::open(path) {
        Ok(mut reader) => reader.verify().is_ok(),
        Err(_) => false,
    }
}

#[test]
fn byte_flip_sweep_never_panics_and_rarely_survives() {
    let dir = tempfile::tempdir().unwrap();
    let valid = build_sample(&dir.path().join("sample.cavs"));
    let target = dir.path().join("mutated.cavs");
    assert!(open_and_verify(&valid, &target), "pristine must verify");

    // Flip every superblock byte, then sample the rest of the file (stride
    // keeps CI fast; the offset varies within each stride window).
    let mut positions: Vec<usize> = (0..cavs_format::SUPERBLOCK_LEN as usize).collect();
    let mut state = 0xF117u64;
    let stride = 37;
    for start in (cavs_format::SUPERBLOCK_LEN as usize..valid.len()).step_by(stride) {
        positions.push(start + (xorshift(&mut state) as usize) % stride.min(valid.len() - start));
    }
    for pos in positions {
        let mut m = valid.clone();
        m[pos] ^= 0xff;
        let survived = open_and_verify(&m, &target);
        // Everything after the superblock is covered by a section hash,
        // a chunk hash, the Merkle root or a bounds check: no flip there
        // may survive. Inside the superblock, only the *unauthenticated
        // metadata* fields (version_minor, flags, uuid, timescale,
        // reserved/padding) may: magic, version, algorithms, section
        // count/offset and file size are all validated.
        assert!(
            !survived || pos < cavs_format::SUPERBLOCK_LEN as usize,
            "byte flip at {pos} survived full verification"
        );
    }
}

#[test]
fn truncations_never_verify() {
    let dir = tempfile::tempdir().unwrap();
    let valid = build_sample(&dir.path().join("sample.cavs"));
    let target = dir.path().join("mutated.cavs");
    for cut in (0..valid.len()).step_by(211) {
        assert!(
            !open_and_verify(&valid[..cut], &target),
            "truncation at {cut} verified"
        );
    }
}

#[test]
fn random_garbage_never_panics() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("garbage.cavs");
    let mut state = 0xBAD_5EEDu64;
    for i in 0..1500 {
        let len = (xorshift(&mut state) % 4096) as usize;
        let mut bytes = vec![0u8; len];
        for b in bytes.iter_mut() {
            *b = xorshift(&mut state) as u8;
        }
        // Half the iterations wear a valid magic to get past the doorman.
        if i % 2 == 0 && bytes.len() >= 4 {
            bytes[..4].copy_from_slice(b"CAVS");
        }
        assert!(!open_and_verify(&bytes, &target));
    }
}
