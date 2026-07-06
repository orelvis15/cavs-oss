//! Deterministic mini-fuzz of the CVSP batch decoders (v0.5.0).
//!
//! CI-friendly replay of the corruption space: seeded PRNG, fixed
//! iteration counts, no nightly. The coverage-guided targets live in
//! `fuzz/` and share these invariants: never panic, never over-allocate,
//! reject malformed input with a structured error.

use cavs_proto::{BatchResponse, DeliveryInstr, InitDelivery, SegmentDelivery};

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

fn sample_batch() -> BatchResponse {
    let hash = |b: u8| [b; 32];
    BatchResponse {
        inits: vec![InitDelivery {
            track_id: 1,
            instrs: vec![
                DeliveryInstr::Ref { hash: hash(1) },
                DeliveryInstr::Inline {
                    hash: hash(2),
                    len_raw: 5,
                    compression: cavs_proto::WIRE_COMPRESSION_NONE,
                    payload: b"hello".to_vec(),
                },
            ],
        }],
        segments: vec![SegmentDelivery {
            segment_id: 7,
            instrs: vec![DeliveryInstr::Inline {
                hash: hash(3),
                len_raw: 3,
                compression: cavs_proto::WIRE_COMPRESSION_NONE,
                payload: b"abc".to_vec(),
            }],
        }],
    }
}

/// A header declaring 4 billion items must fail fast, not pre-allocate.
#[test]
fn hostile_counts_do_not_allocate() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"CVSP");
    bytes.push(2);
    bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // init_count
    assert!(BatchResponse::decode(&bytes).is_err());
    let mut r = bytes.as_slice();
    assert!(cavs_proto::decode_stream(&mut r, |_| Ok(())).is_err());
}

/// An inline instruction declaring a multi-GiB payload must be rejected
/// before the allocation, in both decoders.
#[test]
fn hostile_inline_length_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"CVSP");
    bytes.push(2);
    bytes.extend_from_slice(&1u32.to_le_bytes()); // one init
    bytes.extend_from_slice(&1u32.to_le_bytes()); // track_id
    bytes.extend_from_slice(&1u32.to_le_bytes()); // one instr
    bytes.push(1); // Inline
    bytes.extend_from_slice(&[0u8; 32]); // hash
    bytes.push(0); // compression none
    bytes.extend_from_slice(&16u32.to_le_bytes()); // len_raw
    bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // len_stored: hostile
    assert!(BatchResponse::decode(&bytes).is_err());
    let mut r = bytes.as_slice();
    assert!(cavs_proto::decode_stream(&mut r, |_| Ok(())).is_err());
}

#[test]
fn random_garbage_never_panics() {
    let mut state = 0x5EED_0001u64;
    for _ in 0..4000 {
        let len = (xorshift(&mut state) % 2048) as usize;
        let mut bytes = vec![0u8; len];
        for b in bytes.iter_mut() {
            *b = xorshift(&mut state) as u8;
        }
        let _ = BatchResponse::decode(&bytes);
        let mut r = bytes.as_slice();
        let _ = cavs_proto::decode_stream(&mut r, |_| Ok(()));
    }
}

#[test]
fn every_single_byte_flip_is_handled() {
    let valid = sample_batch().encode();
    // Sanity: the pristine encoding round-trips.
    assert_eq!(BatchResponse::decode(&valid).unwrap(), sample_batch());
    for i in 0..valid.len() {
        let mut m = valid.clone();
        m[i] ^= 0xff;
        // Must not panic; Ok is fine when the flip lands in a payload byte
        // (payload integrity is the chunk hash's job, checked upstream).
        let _ = BatchResponse::decode(&m);
        let mut r = m.as_slice();
        let _ = cavs_proto::decode_stream(&mut r, |_| Ok(()));
    }
}

#[test]
fn every_truncation_is_rejected() {
    let valid = sample_batch().encode();
    for cut in 0..valid.len() {
        assert!(
            BatchResponse::decode(&valid[..cut]).is_err(),
            "truncation at {cut} was accepted"
        );
        let mut r = &valid[..cut];
        assert!(cavs_proto::decode_stream(&mut r, |_| Ok(())).is_err());
    }
}
