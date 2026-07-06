//! Deterministic mini-fuzz of packfile storage decoders (v0.5.0).
//!
//! Every byte of a `.cavsindex` and the integrity-relevant bytes of a
//! `.cavspack` are covered by hashes, so *any* single-byte flip must be
//! rejected — and arbitrary garbage must never panic the readers.

use cavs_hash::hash_chunk;
use cavs_store::packfile;
use std::path::PathBuf;

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

fn build_pack(dir: &std::path::Path) -> (PathBuf, PathBuf) {
    let mut writer = packfile::PackWriter::create(dir).unwrap();
    let mut state = 0xACE0Fu64;
    for i in 0..24u32 {
        let mut data = vec![0u8; 800 + i as usize * 37];
        for b in data.iter_mut() {
            *b = xorshift(&mut state) as u8;
        }
        writer
            .append(hash_chunk(&data), &data, data.len() as u32, 0)
            .unwrap();
    }
    let (hex, _) = writer.finish().unwrap();
    (
        packfile::pack_path(dir, &hex),
        packfile::index_path(dir, &hex),
    )
}

#[test]
fn every_index_byte_flip_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (_, index) = build_pack(dir.path());
    let valid = std::fs::read(&index).unwrap();
    packfile::read_pack_index(&index).unwrap();

    let mutated = dir.path().join("mutated.cavsindex");
    for i in 0..valid.len() {
        let mut m = valid.clone();
        m[i] ^= 0xff;
        std::fs::write(&mutated, &m).unwrap();
        assert!(
            packfile::read_pack_index(&mutated).is_err(),
            "index byte flip at {i} was accepted"
        );
    }
}

#[test]
fn every_pack_byte_flip_fails_verification() {
    let dir = tempfile::tempdir().unwrap();
    let (pack, _) = build_pack(dir.path());
    let valid = std::fs::read(&pack).unwrap();
    packfile::verify_pack(&pack).unwrap();

    let mutated = dir.path().join("mutated.cavspack");
    for i in (0..valid.len()).step_by(97) {
        let mut m = valid.clone();
        m[i] ^= 0xff;
        std::fs::write(&mutated, &m).unwrap();
        assert!(
            packfile::verify_pack(&mutated).is_err(),
            "pack byte flip at {i} passed verification"
        );
    }
}

#[test]
fn garbage_and_truncations_never_panic() {
    let dir = tempfile::tempdir().unwrap();
    let (pack, index) = build_pack(dir.path());
    let pack_bytes = std::fs::read(&pack).unwrap();
    let target_pack = dir.path().join("t.cavspack");
    let target_idx = dir.path().join("t.cavsindex");

    for cut in (0..pack_bytes.len()).step_by(131) {
        std::fs::write(&target_pack, &pack_bytes[..cut]).unwrap();
        assert!(packfile::verify_pack(&target_pack).is_err());
    }
    let idx_bytes = std::fs::read(&index).unwrap();
    for cut in 0..idx_bytes.len() {
        std::fs::write(&target_idx, &idx_bytes[..cut]).unwrap();
        assert!(packfile::read_pack_index(&target_idx).is_err());
    }

    let mut state = 0x600D_BEEFu64;
    for _ in 0..800 {
        let len = (xorshift(&mut state) % 2048) as usize;
        let mut bytes = vec![0u8; len];
        for b in bytes.iter_mut() {
            *b = xorshift(&mut state) as u8;
        }
        std::fs::write(&target_idx, &bytes).unwrap();
        let _ = packfile::read_pack_index(&target_idx);
        std::fs::write(&target_pack, &bytes).unwrap();
        let _ = packfile::verify_pack(&target_pack);
        // Range reads with arbitrary offsets must bounds-check, not panic.
        let off = xorshift(&mut state);
        let _ = packfile::read_pack_range(&target_pack, off, xorshift(&mut state) % 4096);
    }
}
