use cavs_format::{
    Reader, SegmentRecord, TrackKind, TrackRecord, Writer, SEGMENT_FLAG_RANDOM_ACCESS,
};
use std::io::{Read, Seek, SeekFrom, Write};

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
fn full_roundtrip_with_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.cavs");

    let init = pseudo_random(4096, 1);
    let seg_a = pseudo_random(300_000, 2);
    let seg_b = pseudo_random(300_000, 3);
    // seg_c is a byte-for-byte repeat of seg_a: must dedupe fully.
    let seg_c = seg_a.clone();
    let compressible = vec![0x42u8; 100_000];

    let mut w = Writer::create(&path, [7u8; 16], 1000, true).unwrap();

    let init_idx = w.add_chunk(&init).unwrap();
    w.pin_dict(init_idx).unwrap();

    let mut seg_indices = Vec::new();
    for data in [&seg_a, &seg_b, &seg_c, &compressible] {
        let mut idxs = Vec::new();
        for range in cavs_chunker_split(data) {
            idxs.push(w.add_chunk(&data[range]).unwrap());
        }
        seg_indices.push(idxs);
    }

    w.add_track(TrackRecord {
        track_id: 1,
        kind: TrackKind::Video,
        flags: 0,
        codec: "avc1.64001f".into(),
        name: "video".into(),
        timescale: 90_000,
        init_chunks: vec![init_idx],
    })
    .unwrap();

    for (i, idxs) in seg_indices.iter().enumerate() {
        w.add_segment(SegmentRecord {
            segment_id: i as u64,
            track_id: 1,
            pts_start: i as u64 * 4000,
            duration: 4000,
            flags: SEGMENT_FLAG_RANDOM_ACCESS,
            chunks: idxs.clone(),
        })
        .unwrap();
    }
    w.set_meta("source", "roundtrip-test");

    let stats = w.finish().unwrap();
    // seg_c fully deduped against seg_a.
    assert!(stats.logical_raw > stats.unique_raw);
    assert!(stats.logical_chunks > stats.unique_chunks);
    // The zero-filled chunk must have compressed.
    assert!(stats.stored < stats.unique_raw);

    let mut r = Reader::open(&path).unwrap();
    assert_eq!(r.superblock().timescale, 1000);
    assert_eq!(r.tracks().len(), 1);
    assert_eq!(r.segments().len(), 4);
    assert_eq!(r.dict(), &[init_idx]);
    assert_eq!(
        r.meta(),
        &[("source".to_string(), "roundtrip-test".to_string())]
    );

    assert_eq!(r.track_init_bytes(1).unwrap(), init);
    let segs: Vec<SegmentRecord> = r
        .segments_for_track(1)
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(r.segment_bytes(&segs[0]).unwrap(), seg_a);
    assert_eq!(r.segment_bytes(&segs[1]).unwrap(), seg_b);
    assert_eq!(r.segment_bytes(&segs[2]).unwrap(), seg_c);
    assert_eq!(r.segment_bytes(&segs[3]).unwrap(), compressible);

    let report = r.verify().unwrap();
    assert!(report.merkle_ok && report.data_section_ok);
    assert_eq!(report.chunks_verified, stats.unique_chunks);
}

#[test]
fn corruption_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.cavs");

    let payload = pseudo_random(200_000, 9);
    let mut w = Writer::create(&path, [0u8; 16], 1000, false).unwrap();
    let mut idxs = Vec::new();
    for range in cavs_chunker_split(&payload) {
        idxs.push(w.add_chunk(&payload[range]).unwrap());
    }
    w.add_segment(SegmentRecord {
        segment_id: 0,
        track_id: 0,
        pts_start: 0,
        duration: 0,
        flags: 0,
        chunks: idxs,
    })
    .unwrap();
    w.finish().unwrap();

    // Flip one byte inside the DATA section (right after the superblock).
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    f.seek(SeekFrom::Start(64 + 1000)).unwrap();
    let mut b = [0u8; 1];
    f.read_exact(&mut b).unwrap();
    f.seek(SeekFrom::Start(64 + 1000)).unwrap();
    f.write_all(&[b[0] ^ 0xFF]).unwrap();
    drop(f);

    let mut r = Reader::open(&path).unwrap();
    assert!(r.verify().is_err(), "verification must catch bit flips");
}

#[test]
fn bad_magic_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.cavs");
    std::fs::write(&path, vec![0u8; 128]).unwrap();
    assert!(matches!(
        Reader::open(&path),
        Err(cavs_format::FormatError::BadMagic)
    ));
}

// Local fixed-size splitter so this test crate doesn't depend on cavs-chunker.
fn cavs_chunker_split(data: &[u8]) -> Vec<std::ops::Range<usize>> {
    const SIZE: usize = 64 * 1024;
    let mut out = Vec::new();
    let mut off = 0;
    while off < data.len() {
        let end = (off + SIZE).min(data.len());
        out.push(off..end);
        off = end;
    }
    out
}

// --- fuzz-style hardening: malformed files must be rejected, never panic ---

fn valid_file(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("valid.cavs");
    let mut w = Writer::create(&path, [0u8; 16], 1000, false).unwrap();
    let idx = w.add_chunk(&pseudo_random(50_000, 1)).unwrap();
    w.add_segment(SegmentRecord {
        segment_id: 0,
        track_id: 0,
        pts_start: 0,
        duration: 0,
        flags: 0,
        chunks: vec![idx],
    })
    .unwrap();
    w.finish().unwrap();
    path
}

/// Patch 8 little-endian bytes at `off` and confirm the reader errors
/// (Result::Err) instead of panicking or trying a giant allocation.
fn patch_u64_expect_err(src: &std::path::Path, dst: &std::path::Path, off: u64, val: u64) {
    std::fs::copy(src, dst).unwrap();
    let mut f = std::fs::OpenOptions::new().write(true).open(dst).unwrap();
    f.seek(SeekFrom::Start(off)).unwrap();
    f.write_all(&val.to_le_bytes()).unwrap();
    drop(f);
    // Must return an error, and must return quickly (no huge allocation).
    assert!(Reader::open(dst).is_err(), "reader accepted a malformed file");
}

#[test]
fn rejects_absurd_section_count() {
    let dir = tempfile::tempdir().unwrap();
    let src = valid_file(dir.path());
    // section_count is at superblock offset 36 (u32). Blow it up.
    let bad = dir.path().join("bad_count.cavs");
    std::fs::copy(&src, &bad).unwrap();
    let mut f = std::fs::OpenOptions::new().write(true).open(&bad).unwrap();
    f.seek(SeekFrom::Start(36)).unwrap();
    f.write_all(&u32::MAX.to_le_bytes()).unwrap();
    drop(f);
    assert!(Reader::open(&bad).is_err());
}

#[test]
fn rejects_absurd_offsets_and_lengths() {
    let dir = tempfile::tempdir().unwrap();
    let src = valid_file(dir.path());
    // section_dir_offset (u64 at superblock offset 40) far past EOF.
    patch_u64_expect_err(&src, &dir.path().join("b1.cavs"), 40, u64::MAX / 2);
    // section_dir_offset = 0 (points into the superblock: garbage dir).
    patch_u64_expect_err(&src, &dir.path().join("b2.cavs"), 40, 0);
}

#[test]
fn rejects_truncated_files() {
    let dir = tempfile::tempdir().unwrap();
    let src = valid_file(dir.path());
    let full = std::fs::read(&src).unwrap();
    // Every truncation point must error, never panic.
    for cut in [0usize, 4, 32, 64, full.len() / 2, full.len().saturating_sub(1)] {
        let p = dir.path().join(format!("trunc_{cut}.cavs"));
        std::fs::write(&p, &full[..cut]).unwrap();
        assert!(Reader::open(&p).is_err(), "accepted a file truncated to {cut} bytes");
    }
}

#[test]
fn rejects_random_garbage_with_valid_magic() {
    let dir = tempfile::tempdir().unwrap();
    // "CAVS" magic + version, then random bytes. Must not panic.
    for seed in 0..40u32 {
        let mut buf = b"CAVS".to_vec();
        buf.extend_from_slice(&1u16.to_le_bytes()); // major
        buf.extend_from_slice(&pseudo_random(4096, seed.wrapping_add(1)));
        let p = dir.path().join(format!("garbage_{seed}.cavs"));
        std::fs::write(&p, &buf).unwrap();
        let _ = Reader::open(&p); // only requirement: no panic / no hang
    }
}
