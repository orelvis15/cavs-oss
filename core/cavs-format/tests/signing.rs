use cavs_format::{Reader, SegmentRecord, SignatureStatus, Writer};

fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut state = seed;
    for b in out.iter_mut() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (state >> 24) as u8;
    }
    out
}

fn write_signed(path: &std::path::Path, secret: &[u8; 32], payload: &[u8]) {
    let mut w = Writer::create(path, [1u8; 16], 1000, true).unwrap();
    w.sign_with(secret);
    let idx = w.add_chunk(payload).unwrap();
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
}

#[test]
fn signature_roundtrip_and_signer_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signed.cavs");
    let secret = [42u8; 32];
    write_signed(&path, &secret, &pseudo_random(100_000, 5));

    let r = Reader::open(&path).unwrap();
    let expected_pk = ed25519_dalek::SigningKey::from_bytes(&secret)
        .verifying_key()
        .to_bytes();
    assert_eq!(r.verify_signature().unwrap(), SignatureStatus::Valid(expected_pk));
}

#[test]
fn unsigned_file_reports_unsigned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.cavs");
    let mut w = Writer::create(&path, [0u8; 16], 1000, false).unwrap();
    let idx = w.add_chunk(b"data").unwrap();
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

    let r = Reader::open(&path).unwrap();
    assert_eq!(r.verify_signature().unwrap(), SignatureStatus::Unsigned);
}

#[test]
fn tampered_signature_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.cavs");
    let evil = dir.path().join("evil.cavs");
    let secret = [7u8; 32];
    write_signed(&good, &secret, b"original content that gets signed");
    // Attacker repacks different content, then splices the victim's
    // signature meta entries in place of their own. Verification must fail
    // because the merkle root differs.
    write_signed(&evil, &[9u8; 32], b"malicious replacement content!!!");

    let good_r = Reader::open(&good).unwrap();
    let (sig, pk) = good_r.embedded_signature().unwrap();

    // Rebuild evil with the stolen signature embedded as meta.
    let forged = dir.path().join("forged.cavs");
    let mut w = Writer::create(&forged, [1u8; 16], 1000, true).unwrap();
    let idx = w.add_chunk(b"malicious replacement content!!!").unwrap();
    w.add_segment(SegmentRecord {
        segment_id: 0,
        track_id: 0,
        pts_start: 0,
        duration: 0,
        flags: 0,
        chunks: vec![idx],
    })
    .unwrap();
    let sig_hex: String = sig.iter().map(|b| format!("{b:02x}")).collect();
    let pk_hex: String = pk.iter().map(|b| format!("{b:02x}")).collect();
    w.set_meta("sig.ed25519", &sig_hex);
    w.set_meta("sig.pubkey", &pk_hex);
    w.finish().unwrap();

    let r = Reader::open(&forged).unwrap();
    assert!(matches!(
        r.verify_signature(),
        Err(cavs_format::FormatError::SignatureInvalid)
    ));
}
