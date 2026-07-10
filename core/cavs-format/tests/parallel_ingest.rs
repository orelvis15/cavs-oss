//! `Writer::add_chunks_parallel` must be a pure wall-clock optimization:
//! same chunk indices, same dedup decisions, byte-identical output file.

use cavs_format::{Reader, SegmentRecord, TrackKind, TrackRecord, Writer, SEGMENT_FLAG_RANDOM_ACCESS};
use std::ops::Range;
use std::path::Path;

fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut state = seed;
    for b in out.iter_mut() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (state >> 24) as u8;
    }
    out
}

/// A payload exercising every ingest decision: compressible runs,
/// incompressible noise, exact duplicates (intra-batch dedup), and chunks
/// below the 512-byte compression floor.
fn tricky_payload() -> (Vec<u8>, Vec<Range<usize>>) {
    let mut data = Vec::new();
    let mut ranges = Vec::new();
    let mut push = |data: &mut Vec<u8>, ranges: &mut Vec<Range<usize>>, bytes: &[u8]| {
        let start = data.len();
        data.extend_from_slice(bytes);
        ranges.push(start..data.len());
    };
    let noise = pseudo_random(70_000, 9);
    let compressible = vec![0x5a_u8; 50_000];
    push(&mut data, &mut ranges, &compressible); // compresses
    push(&mut data, &mut ranges, &noise); // stays raw
    push(&mut data, &mut ranges, &compressible); // intra-batch duplicate
    push(&mut data, &mut ranges, &noise[..300]); // below COMPRESS_MIN_LEN
    push(&mut data, &mut ranges, b"tiny"); // 4 bytes
    push(&mut data, &mut ranges, &pseudo_random(200_000, 10)); // large raw
    let mut half = compressible.clone();
    half.extend_from_slice(&noise[..20_000]); // mixed compressibility
    push(&mut data, &mut ranges, &half);
    (data, ranges)
}

fn write_with(
    path: &Path,
    data: &[u8],
    ranges: &[Range<usize>],
    parallel: bool,
    zstd_level: i32,
) -> (Vec<u32>, cavs_format::PackStats) {
    let mut w = Writer::create(path, [3u8; 16], 1000, true).unwrap();
    w.set_zstd_level(zstd_level);
    let idxs = if parallel {
        w.add_chunks_parallel(data, ranges).unwrap()
    } else {
        ranges
            .iter()
            .map(|r| w.add_chunk(&data[r.clone()]).unwrap())
            .collect()
    };
    w.add_track(TrackRecord {
        track_id: 1,
        kind: TrackKind::Data,
        flags: 0,
        codec: "raw".into(),
        name: "payload".into(),
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
        chunks: idxs.clone(),
    })
    .unwrap();
    (idxs, w.finish().unwrap())
}

#[test]
fn parallel_ingest_is_byte_identical_to_serial() {
    let dir = tempfile::tempdir().unwrap();
    let (data, ranges) = tricky_payload();

    for level in [3, 19] {
        let serial_path = dir.path().join(format!("serial-{level}.cavs"));
        let parallel_path = dir.path().join(format!("parallel-{level}.cavs"));
        let (idx_s, stats_s) = write_with(&serial_path, &data, &ranges, false, level);
        let (idx_p, stats_p) = write_with(&parallel_path, &data, &ranges, true, level);

        assert_eq!(idx_s, idx_p, "chunk indices must match (level {level})");
        assert_eq!(stats_s.unique_chunks, stats_p.unique_chunks);
        assert_eq!(stats_s.logical_chunks, stats_p.logical_chunks);
        assert_eq!(stats_s.stored, stats_p.stored);
        assert_eq!(stats_s.merkle_root, stats_p.merkle_root);

        let a = std::fs::read(&serial_path).unwrap();
        let b = std::fs::read(&parallel_path).unwrap();
        assert_eq!(a, b, "files must be byte-identical (level {level})");
    }
}

#[test]
fn parallel_ingest_dedups_across_batches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cross-batch.cavs");
    let noise = pseudo_random(80_000, 21);
    let mut w = Writer::create(&path, [3u8; 16], 1000, true).unwrap();

    // First batch ingests the payload; the second is a full repeat and must
    // resolve every chunk to the already-stored indices.
    let ranges = vec![0..30_000, 30_000..80_000];
    let first = w.add_chunks_parallel(&noise, &ranges).unwrap();
    let second = w.add_chunks_parallel(&noise, &ranges).unwrap();
    assert_eq!(first, second);

    w.add_track(TrackRecord {
        track_id: 1,
        kind: TrackKind::Data,
        flags: 0,
        codec: "raw".into(),
        name: "n".into(),
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
        chunks: first.iter().chain(&second).copied().collect(),
    })
    .unwrap();
    let stats = w.finish().unwrap();
    assert_eq!(stats.unique_chunks, 2);
    assert_eq!(stats.logical_chunks, 4);

    // And the container still reads back clean.
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.chunks().len(), 2);
}
