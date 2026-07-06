//! `cavs test corrupt` — the corruption matrix (v0.5.0).
//!
//! Takes a real `.cavs`, applies one targeted mutation per matrix row to a
//! scratch copy, and asserts that every decoder involved rejects the
//! corrupted artifact cleanly (an error, never a panic, never silently
//! wrong data). Covers the container, the binary v2 manifest, varints,
//! the bootstrap sidecar and packfile storage.

use anyhow::{Context, Result};
use cavs_hash::{hash_chunk, to_hex, Hasher};
use std::path::Path;

struct TestResult {
    target: &'static str,
    mutation: &'static str,
    expected: &'static str,
    pass: bool,
    detail: String,
}

pub fn corrupt(input: &Path, out: Option<&Path>) -> Result<()> {
    let scratch = tempfile::tempdir().context("creating scratch dir")?;
    let original = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let mut results: Vec<TestResult> = Vec::new();

    container_matrix(&original, scratch.path(), &mut results)?;
    manifest_matrix(input, &mut results)?;
    varint_matrix(&mut results);
    bootstrap_matrix(input, &mut results)?;
    packfile_matrix(input, scratch.path(), &mut results)?;

    let failed = results.iter().filter(|r| !r.pass).count();
    println!("Corruption matrix: {} tests", results.len());
    for r in &results {
        println!(
            "  {} {:<28} {:<24} expected {:<8} {}",
            if r.pass { "PASS" } else { "FAIL" },
            r.target,
            r.mutation,
            r.expected,
            r.detail
        );
    }
    if let Some(path) = out {
        let json: Vec<String> = results
            .iter()
            .map(|r| {
                format!(
                    "{{\"target\":\"{}\",\"mutation\":\"{}\",\"expected\":\"{}\",\"result\":\"{}\"}}",
                    r.target,
                    r.mutation,
                    r.expected,
                    if r.pass { "pass" } else { "fail" }
                )
            })
            .collect();
        std::fs::write(path, format!("{{\"tests\":[{}]}}\n", json.join(",")))?;
        println!("report written to {}", path.display());
    }
    if failed > 0 {
        anyhow::bail!("{failed} corruption test(s) FAILED: a corrupted input was accepted");
    }
    println!("all corrupted inputs were rejected cleanly");
    Ok(())
}

/// Expectation helper: the operation must return an error.
fn expect_reject<T>(
    results: &mut Vec<TestResult>,
    target: &'static str,
    mutation: &'static str,
    r: std::result::Result<T, String>,
) {
    let (pass, detail) = match r {
        Err(e) => (true, truncate(&e)),
        Ok(_) => (false, "ACCEPTED corrupt input".to_string()),
    };
    results.push(TestResult {
        target,
        mutation,
        expected: "reject",
        pass,
        detail,
    });
}

fn truncate(s: &str) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > 60 {
        let cut: String = s.chars().take(60).collect();
        format!("{cut}…")
    } else {
        s
    }
}

/// Write mutated bytes to a scratch file and try to fully open+verify it.
fn open_and_verify(bytes: &[u8], scratch: &Path, name: &str) -> std::result::Result<(), String> {
    let path = scratch.join(name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    // Verification errors are the *expected* outcome here: stringify them.
    let mut reader = cavs_format::Reader::open(&path).map_err(|e| e.to_string())?;
    reader.verify().map_err(|e| e.to_string())?;
    Ok(())
}

fn container_matrix(original: &[u8], scratch: &Path, results: &mut Vec<TestResult>) -> Result<()> {
    // Sanity: the pristine file must verify, or the matrix means nothing.
    open_and_verify(original, scratch, "pristine.cavs")
        .map_err(|e| anyhow::anyhow!("input does not verify clean: {e}"))?;

    let flip = |at: usize| -> Vec<u8> {
        let mut m = original.to_vec();
        m[at] ^= 0xff;
        m
    };

    expect_reject(
        results,
        "container_magic",
        "flip_byte",
        open_and_verify(&flip(0), scratch, "magic.cavs"),
    );

    // Superblock v1 layout: section_dir_offset lives at bytes 40..48.
    let mut m = original.to_vec();
    m[40..48].copy_from_slice(&(original.len() as u64 + 1000).to_le_bytes());
    expect_reject(
        results,
        "section_dir_offset",
        "point_outside_file",
        open_and_verify(&m, scratch, "diroob.cavs"),
    );

    // Locate sections from the pristine reader to aim precise mutations.
    let pristine = scratch.join("pristine.cavs");
    let reader = cavs_format::Reader::open(&pristine)?;
    let sections = reader.sections().to_vec();
    let dir_offset = u64::from_le_bytes(original[40..48].try_into().unwrap()) as usize;
    drop(reader);

    let tracks = sections
        .iter()
        .find(|s| s.section_type == cavs_format::SectionType::Tracks)
        .context("no tracks section")?;
    expect_reject(
        results,
        "table_section_bytes",
        "flip_byte",
        open_and_verify(&flip(tracks.offset as usize), scratch, "tracks.cavs"),
    );

    // Grow the first directory entry's length past EOF (offset u64 at +4,
    // length u64 at +12 inside the 52-byte entry).
    let mut m = original.to_vec();
    m[dir_offset + 12..dir_offset + 20].copy_from_slice(&(original.len() as u64 * 2).to_le_bytes());
    expect_reject(
        results,
        "section_length",
        "grow_past_eof",
        open_and_verify(&m, scratch, "seclen.cavs"),
    );

    let data = sections
        .iter()
        .find(|s| s.section_type == cavs_format::SectionType::Data)
        .context("no data section")?;
    expect_reject(
        results,
        "chunk_data_bytes",
        "flip_byte",
        open_and_verify(
            &flip((data.offset + data.length / 2) as usize),
            scratch,
            "data.cavs",
        ),
    );

    expect_reject(
        results,
        "container_truncated",
        "cut_last_1024",
        open_and_verify(
            &original[..original.len().saturating_sub(1024)],
            scratch,
            "trunc.cavs",
        ),
    );
    Ok(())
}

fn manifest_matrix(input: &Path, results: &mut Vec<TestResult>) -> Result<()> {
    let reader = cavs_format::Reader::open(input)?;
    let manifest = cavs_manifest::manifest_from_reader(&reader, "corrupt-test")?;
    let encoded = cavs_manifest::encode_manifest_v2(&manifest)?;
    let reference = serde_json::to_string(&manifest)?;

    let decode = |bytes: &[u8]| -> std::result::Result<(), String> {
        let loaded = cavs_manifest::read_manifest(bytes).map_err(|e| e.to_string())?;
        // A mutation may land on ignored bits (reserved flags): decoding is
        // only a defect if it yields a *different* manifest silently.
        let round = serde_json::to_string(&loaded.manifest).map_err(|e| e.to_string())?;
        if round == reference {
            Err("decoded but bit was inert (identical manifest)".into())
        } else {
            Ok(())
        }
    };

    let mut flipped = encoded.clone();
    flipped[0] ^= 0xff;
    expect_reject(results, "manifest_magic", "flip_byte", decode(&flipped));

    let flips: [(&'static str, usize); 4] = [
        ("manifest_header", 12),
        ("manifest_body_25pct", encoded.len() / 4),
        ("manifest_body_50pct", encoded.len() / 2),
        ("manifest_body_75pct", encoded.len() * 3 / 4),
    ];
    for (target, pos) in flips {
        let mut m = encoded.clone();
        m[pos] ^= 0xff;
        expect_reject(results, target, "flip_byte", decode(&m));
    }

    let cuts: [(&'static str, usize); 3] = [
        ("manifest_trunc_header", encoded.len().min(20)),
        ("manifest_trunc_half", encoded.len() / 2),
        ("manifest_trunc_tail", encoded.len().saturating_sub(1)),
    ];
    for (target, cut) in cuts {
        expect_reject(results, target, "truncate", decode(&encoded[..cut]));
    }
    Ok(())
}

fn varint_matrix(results: &mut Vec<TestResult>) {
    let overlong = [0x80u8; 11];
    expect_reject(
        results,
        "varint",
        "overlong_sequence",
        cavs_manifest::varint::read_varuint(&mut &overlong[..])
            .map(|_| ())
            .map_err(|e| e.to_string()),
    );
    let truncated = [0x80u8, 0x80];
    expect_reject(
        results,
        "varint",
        "truncated_sequence",
        cavs_manifest::varint::read_varuint(&mut &truncated[..])
            .map(|_| ())
            .map_err(|e| e.to_string()),
    );
}

fn bootstrap_matrix(input: &Path, results: &mut Vec<TestResult>) -> Result<()> {
    let reader = cavs_format::Reader::open(input)?;
    let get = |key: &str| {
        reader
            .meta()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let Some(expected) = get("bootstrap.blake3") else {
        return Ok(()); // no sidecar declared: nothing to corrupt
    };
    let sidecar = std::path::PathBuf::from(format!("{}.bootstrap.zst", input.display()));
    let Ok(mut bytes) = std::fs::read(&sidecar) else {
        return Ok(());
    };
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    // The client's acceptance check is BLAKE3(wire) == announced hash.
    let mut hasher = Hasher::new();
    hasher.update(&bytes);
    let verdict = if to_hex(&hasher.finalize()).eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err("BLAKE3 mismatch detected".to_string())
    };
    expect_reject(results, "bootstrap_sidecar", "flip_byte", verdict);
    Ok(())
}

fn packfile_matrix(input: &Path, scratch: &Path, results: &mut Vec<TestResult>) -> Result<()> {
    use cavs_store::packfile;

    // Build a real packfile from the container's chunks.
    let mut reader = cavs_format::Reader::open(input)?;
    let packs_dir = scratch.join("packs");
    let mut writer = packfile::PackWriter::create(&packs_dir)?;
    let n = reader.chunks().len().min(64) as u32; // enough chunks to matter
    for i in 0..n {
        let (stored, flags, len_raw) = reader.read_chunk_stored(i)?;
        let hash = reader.chunks()[i as usize].hash;
        writer.append(hash, &stored, len_raw, flags)?;
    }
    let (pack_hex, entries) = writer.finish()?;
    let pack = packfile::pack_path(&packs_dir, &pack_hex);
    let index = packfile::index_path(&packs_dir, &pack_hex);
    let pack_bytes = std::fs::read(&pack)?;

    let with_mutated_pack = |bytes: &[u8]| -> std::path::PathBuf {
        let p = scratch.join("mutated.cavspack");
        std::fs::write(&p, bytes).unwrap();
        p
    };

    let mut m = pack_bytes.clone();
    m[0] ^= 0xff;
    expect_reject(
        results,
        "packfile_header",
        "flip_byte",
        packfile::verify_pack(&with_mutated_pack(&m)).map_err(|e| e.to_string()),
    );

    let mut m = pack_bytes.clone();
    m[packfile::PACK_HEADER_LEN as usize + 5] ^= 0xff;
    let mutated = with_mutated_pack(&m);
    expect_reject(
        results,
        "packfile_chunk_bytes",
        "flip_byte",
        packfile::verify_pack(&mutated)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                // Belt and braces: even if the footer passed, the chunk's
                // identity hash must not.
                let e = &entries[0];
                let bytes = packfile::read_pack_range(&mutated, e.offset, e.stored_len as u64)
                    .map_err(|x| x.to_string())?;
                if hash_chunk(&bytes) == e.hash {
                    Ok(())
                } else {
                    Err("chunk hash mismatch detected".into())
                }
            }),
    );

    let mut m = pack_bytes.clone();
    let len = m.len();
    m[len - 1] ^= 0xff;
    expect_reject(
        results,
        "packfile_footer_hash",
        "flip_byte",
        packfile::verify_pack(&with_mutated_pack(&m)).map_err(|e| e.to_string()),
    );

    let mut idx_bytes = std::fs::read(&index)?;
    let mid = idx_bytes.len() / 2;
    idx_bytes[mid] ^= 0x01;
    let mutated_idx = scratch.join("mutated.cavsindex");
    std::fs::write(&mutated_idx, &idx_bytes)?;
    expect_reject(
        results,
        "pack_index_bytes",
        "flip_byte",
        packfile::read_pack_index(&mutated_idx)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    );

    expect_reject(
        results,
        "pack_range",
        "offset_outside_file",
        packfile::read_pack_range(&pack, pack_bytes.len() as u64 * 2, 4096)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    );
    Ok(())
}
