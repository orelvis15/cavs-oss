//! BG4 byte-grouping pretransform: transform round-trips, the writer picks
//! it (only) where it pays, and files carrying BG4 chunks read back intact
//! through both the serial and parallel ingest paths.

use cavs_format::{
    bg4_group, bg4_ungroup, Reader, SegmentRecord, TrackKind, TrackRecord, Writer,
    CHUNK_FLAG_BG4, CHUNK_FLAG_ZSTD, SEGMENT_FLAG_RANDOM_ACCESS,
};

#[test]
fn bg4_roundtrip_all_remainders() {
    // Cover every length mod 4, empty, and sub-lane lengths.
    for len in [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 1023, 1024, 1025, 1026, 65_537] {
        let raw: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let grouped = bg4_group(&raw);
        assert_eq!(grouped.len(), raw.len());
        assert_eq!(bg4_ungroup(&grouped), raw, "len {len}");
    }
}

#[test]
fn bg4_groups_lanes() {
    let raw = [0u8, 1, 2, 3, 10, 11, 12, 13, 20];
    assert_eq!(bg4_group(&raw), [0, 10, 20, 1, 11, 2, 12, 3, 13]);
}

/// A float32 random walk: adjacent values share exponent/high bytes, so the
/// byte-planes compress far better grouped than interleaved — the payload
/// BG4 exists for (model weights, vertex buffers, audio samples).
fn float_walk(n: usize, mut state: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(n * 4);
    let mut v = 1000.0f32;
    for _ in 0..n {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        v += (state >> 16) as f32 / 65_536.0 - 0.5;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn write_one_track(path: &std::path::Path, payloads: &[&[u8]], bg4: bool, parallel: bool) {
    let mut w = Writer::create(path, [9u8; 16], 1000, true).unwrap();
    w.set_bg4(bg4);
    let mut chunks = Vec::new();
    if parallel {
        let data: Vec<u8> = payloads.concat();
        let mut ranges = Vec::new();
        let mut off = 0;
        for p in payloads {
            ranges.push(off..off + p.len());
            off += p.len();
        }
        chunks = w.add_chunks_parallel(&data, &ranges).unwrap();
    } else {
        for p in payloads {
            chunks.push(w.add_chunk(p).unwrap());
        }
    }
    w.add_track(TrackRecord {
        track_id: 1,
        kind: TrackKind::Data,
        flags: 0,
        codec: "raw".into(),
        name: "t".into(),
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
}

#[test]
fn writer_picks_bg4_for_numeric_and_zstd_for_text() {
    let dir = tempfile::tempdir().unwrap();
    let numeric = float_walk(16_384, 7); // 64 KiB
    let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog\n"
        .repeat(1500)
        .to_vec();

    for parallel in [false, true] {
        let path = dir.path().join(format!("t{parallel}.cavs"));
        write_one_track(&path, &[&numeric, &text], true, parallel);

        let mut r = Reader::open(&path).unwrap();
        let recs = r.chunks().to_vec();
        assert_eq!(recs[0].flags, CHUNK_FLAG_ZSTD | CHUNK_FLAG_BG4);
        assert!(
            recs[0].len_stored < numeric.len() as u32 * 9 / 10,
            "bg4+zstd must beat 90% on the float walk (got {} of {})",
            recs[0].len_stored,
            numeric.len()
        );
        // Plain text: zstd alone already wins, no BG4 attempt taken.
        assert_eq!(recs[1].flags, CHUNK_FLAG_ZSTD);

        // Both decode and re-verify against their BLAKE3 identity.
        assert_eq!(r.read_chunk(0).unwrap(), numeric);
        assert_eq!(r.read_chunk(1).unwrap(), text);
    }
}

#[test]
fn bg4_off_by_default_and_incompressible_stays_raw() {
    let dir = tempfile::tempdir().unwrap();
    let numeric = float_walk(16_384, 7);
    // High-entropy payload: nothing should pay, with or without BG4.
    let mut state = 1u32;
    let random: Vec<u8> = (0..65_536)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 24) as u8
        })
        .collect();

    let path = dir.path().join("default.cavs");
    write_one_track(&path, &[&numeric, &random], false, false);
    let mut r = Reader::open(&path).unwrap();
    let recs = r.chunks().to_vec();
    assert_eq!(
        recs[0].flags & CHUNK_FLAG_BG4,
        0,
        "bg4 must be opt-in (readers elsewhere may predate the flag)"
    );
    assert_eq!(recs[1].flags, 0, "incompressible chunk stored raw");
    assert_eq!(r.read_chunk(0).unwrap(), numeric);
    assert_eq!(r.read_chunk(1).unwrap(), random);
}
