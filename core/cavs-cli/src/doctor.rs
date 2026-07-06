//! `cavs doctor` — production diagnostics (v0.5.0).
//!
//! One command that answers "is this deployment healthy?": it checks a
//! `.cavs` container (structure, every chunk hash, Merkle root, manifest
//! encoding, bootstrap sidecar, signature), a global store (ledger and
//! packfile consistency) and/or a client cache (corrupt entries). Read-only:
//! it never mutates what it inspects; findings carry `CAVS-E-*` codes.

use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_hash::{hash_chunk, to_hex, Hasher};
use cavs_proto::errors::ErrorCode;
use std::io::Read as _;
use std::path::Path;

pub fn doctor(input: Option<&Path>, store: Option<&Path>, cache: Option<&Path>) -> Result<()> {
    if input.is_none() && store.is_none() && cache.is_none() {
        bail!("nothing to check: pass a .cavs file, --store <dir> and/or --cache <dir>");
    }
    println!("CAVS Doctor");
    let mut problems = 0u32;
    if let Some(path) = input {
        problems += doctor_container(path);
    }
    if let Some(dir) = store {
        problems += doctor_store(dir);
    }
    if let Some(dir) = cache {
        problems += doctor_cache(dir);
    }
    if problems == 0 {
        println!("Result: OK");
        Ok(())
    } else {
        println!("Result: FAIL ({problems} problem(s))");
        bail!("doctor found {problems} problem(s)");
    }
}

/// Check one `.cavs`: structure, chunks, Merkle, manifest, sidecar, signature.
fn doctor_container(path: &Path) -> u32 {
    let mut problems = 0u32;
    let mut reader = match cavs_format::Reader::open(path) {
        Ok(r) => r,
        Err(e) => {
            println!(
                "Container: FAIL — {}",
                ErrorCode::ContainerCorrupt.msg(format!("{}: {e}", path.display()))
            );
            return 1;
        }
    };

    // Duplicate chunk-table entries would break dedup accounting.
    let mut seen = std::collections::HashSet::new();
    let dups = reader
        .chunks()
        .iter()
        .filter(|c| !seen.insert(c.hash))
        .count();

    match reader.verify() {
        Ok(report) => println!(
            "Container: OK {} — {} chunks ({}) verified, Merkle OK{}",
            path.display(),
            report.chunks_verified,
            human_bytes(report.bytes_verified),
            if dups == 0 {
                String::new()
            } else {
                format!(", {dups} duplicate chunk entries")
            }
        ),
        Err(e) => {
            println!(
                "Container: FAIL — {}",
                ErrorCode::ContainerCorrupt.msg(format!("{}: {e}", path.display()))
            );
            problems += 1;
        }
    }
    if dups > 0 {
        problems += 1;
    }

    match reader.verify_signature() {
        Ok(cavs_format::SignatureStatus::Unsigned) => println!("Signature: none"),
        Ok(cavs_format::SignatureStatus::Valid(pk)) => {
            println!("Signature: OK (signer {})", &to_hex(&pk)[..16])
        }
        Err(e) => {
            println!("Signature: FAIL — {}", ErrorCode::SignatureInvalid.msg(e));
            problems += 1;
        }
    }

    // The runtime manifest must encode and decode in both wire formats.
    match cavs_manifest::manifest_from_reader(&reader, "doctor") {
        Ok(manifest) => match cavs_manifest::encode_manifest_v2(&manifest)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                cavs_manifest::read_manifest(&bytes)?;
                Ok(bytes.len())
            }) {
            Ok(len) => println!(
                "Manifest: OK binary-v2 ({}) / json-v1",
                human_bytes(len as u64)
            ),
            Err(e) => {
                println!("Manifest: FAIL — {}", ErrorCode::ManifestCorrupt.msg(e));
                problems += 1;
            }
        },
        Err(e) => {
            println!("Manifest: FAIL — {}", ErrorCode::ManifestCorrupt.msg(e));
            problems += 1;
        }
    }

    problems += doctor_bootstrap(path, reader.meta());
    problems
}

/// Verify the bootstrap sidecar against the container's declared size/hash.
fn doctor_bootstrap(cavs_path: &Path, meta: &[(String, String)]) -> u32 {
    let get = |key: &str| meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let (Some(size), Some(expected)) = (get("bootstrap.size"), get("bootstrap.blake3")) else {
        println!("Bootstrap: none declared");
        return 0;
    };
    let sidecar = std::path::PathBuf::from(format!("{}.bootstrap.zst", cavs_path.display()));
    let fail = |detail: String| {
        println!(
            "Bootstrap: FAIL — {}",
            ErrorCode::BootstrapHashMismatch.msg(detail)
        );
        1
    };
    let Ok(meta_fs) = std::fs::metadata(&sidecar) else {
        println!(
            "Bootstrap: MISSING — {} declared but {} not found (bootstrap route disabled)",
            human_bytes(size.parse().unwrap_or(0)),
            sidecar.display()
        );
        return 1;
    };
    if size.parse::<u64>().ok() != Some(meta_fs.len()) {
        return fail(format!(
            "{}: size {} does not match declared {size}",
            sidecar.display(),
            meta_fs.len()
        ));
    }
    let mut hasher = Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let Ok(mut file) = std::fs::File::open(&sidecar) else {
        return fail(format!("cannot open {}", sidecar.display()));
    };
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) => return fail(format!("reading {}: {e}", sidecar.display())),
        }
    }
    if !to_hex(&hasher.finalize()).eq_ignore_ascii_case(expected) {
        return fail(format!("{}: BLAKE3 mismatch", sidecar.display()));
    }
    println!(
        "Bootstrap: OK {} ({})",
        sidecar.display(),
        human_bytes(meta_fs.len())
    );
    0
}

/// Check a global store: layout, ledger consistency, every chunk, packs.
fn doctor_store(dir: &Path) -> u32 {
    let store = match cavs_store::GlobalStore::open(dir) {
        Ok(s) => s,
        Err(e) => {
            println!(
                "Store: FAIL — {}",
                ErrorCode::PackCorrupt.msg(format!("{}: {e}", dir.display()))
            );
            return 1;
        }
    };
    let mut problems = 0u32;
    let stats = store.stats();
    println!(
        "Store: {} — {} assets, {} chunks, layout {:?}",
        dir.display(),
        stats.assets,
        stats.unique_chunks,
        store.layout()
    );

    // Every chunk an asset references must resolve in the ledger.
    let mut missing = 0u64;
    for name in store.asset_names() {
        let Ok(asset) = store.get_asset(&name) else {
            println!("Store: FAIL — asset {name} unreadable");
            problems += 1;
            continue;
        };
        let all = asset
            .tracks
            .iter()
            .flat_map(|t| t.init_chunks.iter())
            .chain(asset.segments.iter().flat_map(|s| s.chunks.iter()));
        for hex in all {
            let resolvable = cavs_hash::from_hex(hex)
                .map(|h| store.has_chunk(&h))
                .unwrap_or(false);
            if !resolvable {
                missing += 1;
            }
        }
    }
    if missing > 0 {
        println!(
            "Store: FAIL — {}",
            ErrorCode::ChunkHashMismatch.msg(format!("{missing} referenced chunks missing"))
        );
        problems += 1;
    } else {
        println!("Missing chunks: 0");
    }

    match store.verify() {
        Ok(n) => println!("Chunks: OK — {n} verified (loose and packed, pack integrity checked)"),
        Err(e) => {
            println!("Chunks: FAIL — {}", ErrorCode::PackCorrupt.msg(e));
            problems += 1;
        }
    }
    problems
}

/// Scan a client chunk cache for corrupt entries. Read-only: corruption
/// here is recoverable (`cavs-client cache verify` quarantines and the
/// next fetch re-downloads), so it is reported but does not fail doctor.
fn doctor_cache(dir: &Path) -> u32 {
    let mut total = 0u64;
    let mut corrupt = 0u64;
    let Ok(shards) = std::fs::read_dir(dir) else {
        println!("Cache: FAIL — cannot read {}", dir.display());
        return 1;
    };
    for shard in shards.flatten() {
        let path = shard.path();
        let is_shard = path.is_dir()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_hexdigit()));
        if !is_shard {
            continue;
        }
        for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            total += 1;
            let ok = std::fs::read(&p)
                .map(|payload| to_hex(&hash_chunk(&payload)) == name)
                .unwrap_or(false);
            if !ok {
                corrupt += 1;
            }
        }
    }
    if corrupt == 0 {
        println!("Cache: OK — {total} chunks verified");
    } else {
        println!(
            "Cache: {} — {corrupt} of {total} entries corrupt; run `cavs-client cache verify` to quarantine them",
            ErrorCode::CacheCorruptRecoverable
        );
    }
    0
}
