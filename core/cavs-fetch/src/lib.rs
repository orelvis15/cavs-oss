//! `cavs-fetch` — an embeddable serverless/CDN fetch engine for CAVS.
//!
//! A launcher or game links this library to install and update a build
//! **in-process**, straight from a static export produced by
//! `cavs store export --static-plans` (S3 / R2 / GitHub Pages / nginx / a
//! local folder) — no `cavs-server` and no shelling out to the CLI. It:
//!
//! 1. reads the per-asset `manifest.json` (reconstruction structure) and
//!    `chunk-map.json` (each chunk's pack + absolute byte range),
//! 2. computes the missing set against a persistent content-addressable
//!    cache (so an update downloads only what changed),
//! 3. HTTP-Range-GETs (or slice-reads, for a local folder) the missing
//!    chunks concurrently, verifying every one by BLAKE3, and
//! 4. reconstructs the output files from the cache — byte-identical or it
//!    fails.
//!
//! It reports progress through a callback and supports cooperative
//! cancellation, so a UI can show a progress bar and a Cancel button. The
//! same engine is exposed through the CAVS SDKs (`fetchStatic`) and the C
//! ABI, which is what the Unity and Unreal plugins call.

mod cache;
mod reconstruct;
mod source;

pub use cache::ChunkCache;
pub use source::StaticSource;

use anyhow::{bail, Context, Result};
use cavs_hash::{from_hex, hash_chunk, ChunkHash};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

const CHUNK_FLAG_ZSTD: u32 = 1;

/// BG4 pretransform chunk flag (mirrors `cavs_format::CHUNK_FLAG_BG4`).
const CHUNK_FLAG_BG4: u32 = 1 << 1;

/// Inverse of the BG4 byte-grouping pretransform (mirrors
/// `cavs_format::bg4_ungroup`; duplicated to keep this crate embeddable
/// without a cavs-format dependency).
fn bg4_ungroup(grouped: &[u8]) -> Vec<u8> {
    let len = grouped.len();
    let mut out = vec![0u8; len];
    let mut it = grouped.iter();
    for lane in 0..4 {
        let mut i = lane;
        while i < len {
            out[i] = *it.next().unwrap();
            i += 4;
        }
    }
    out
}

/// Options for a serverless fetch.
pub struct FetchOptions<'a> {
    /// Concurrent range requests (>=1).
    pub connections: usize,
    /// Optional Ed25519 public key (64 hex) to enforce the content signature.
    pub pubkey: Option<String>,
    /// Progress callback: invoked with cumulative `(done_bytes, total_bytes)`
    /// as chunks land. `total_bytes` is the wire size of the missing set.
    pub progress: Option<&'a (dyn Fn(u64, u64) + Send + Sync)>,
    /// Cooperative cancellation: when set to `true`, an in-flight fetch stops
    /// and returns [`FetchError::Cancelled`].
    pub cancel: Option<&'a AtomicBool>,
}

impl Default for FetchOptions<'_> {
    fn default() -> Self {
        Self {
            connections: 8,
            pubkey: None,
            progress: None,
            cancel: None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct FetchStats {
    /// Bytes pulled over the wire (stored, possibly compressed).
    pub wire_bytes: u64,
    /// Decompressed bytes written to the cache.
    pub raw_bytes: u64,
    /// Chunks downloaded.
    pub fetched: u64,
    /// Chunks already present in the cache (an update's reuse).
    pub reused: u64,
    /// Total logical size of the reconstructed asset.
    pub logical_bytes: u64,
}

/// A fetch failure with a stable reason, so an embedder can decide
/// retry/repair/cancel without parsing prose.
#[derive(Debug)]
pub enum FetchError {
    Cancelled,
    Other(anyhow::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Cancelled => write!(f, "CAVS-E-CANCELLED: fetch cancelled"),
            FetchError::Other(e) => write!(f, "{e:#}"),
        }
    }
}
impl std::error::Error for FetchError {}
impl From<anyhow::Error> for FetchError {
    fn from(e: anyhow::Error) -> Self {
        FetchError::Other(e)
    }
}

#[derive(Debug, Deserialize)]
struct ChunkMapFile {
    #[allow(dead_code)]
    asset: String,
    chunks: Vec<ChunkMapEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct ChunkMapEntry {
    hash: String,
    len_raw: u32,
    len_stored: u32,
    flags: u32,
    pack: String,
    pack_offset_abs: u64,
}

/// Fetch `asset` from the static tree at `source` into `output`, caching in
/// `cache_dir`. Returns egress/reuse stats. Byte-identical reconstruction or
/// an error — a partially written output file is never promoted.
pub fn fetch_static(
    source: &StaticSource,
    asset: &str,
    output: &Path,
    cache_dir: &Path,
    opts: &FetchOptions,
) -> std::result::Result<FetchStats, FetchError> {
    fetch_static_inner(source, asset, output, cache_dir, opts).map_err(|e| {
        // Preserve an explicit cancellation as such.
        if e.downcast_ref::<Cancelled>().is_some() {
            FetchError::Cancelled
        } else {
            FetchError::Other(e)
        }
    })
}

struct Cancelled;
impl std::fmt::Debug for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelled")
    }
}
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelled")
    }
}
impl std::error::Error for Cancelled {}

fn fetch_static_inner(
    source: &StaticSource,
    asset: &str,
    output: &Path,
    cache_dir: &Path,
    opts: &FetchOptions,
) -> Result<FetchStats> {
    let cache = ChunkCache::open(cache_dir)?;

    // 1. Manifest + chunk-map.
    let manifest_bytes = source
        .get_all(&format!("assets/{asset}/manifest.json"))
        .with_context(|| format!("asset {asset}: no manifest.json in the static tree"))?;
    let manifest: cavs_proto::Manifest =
        serde_json::from_slice(&manifest_bytes).context("parsing manifest.json")?;

    if let Some(pk) = &opts.pubkey {
        verify_signature(&manifest, pk)?;
    }

    let map_bytes = source
        .get_all(&format!("assets/{asset}/chunk-map.json"))
        .with_context(|| format!("asset {asset}: no chunk-map.json in the static tree"))?;
    let map: ChunkMapFile = serde_json::from_slice(&map_bytes).context("parsing chunk-map.json")?;
    let locations: HashMap<String, ChunkMapEntry> = map
        .chunks
        .into_iter()
        .map(|c| (c.hash.clone(), c))
        .collect();

    // 2. Missing set.
    let mut seen = std::collections::HashSet::new();
    let mut missing: Vec<ChunkMapEntry> = Vec::new();
    let mut reused = 0u64;
    for hex in manifest_chunk_hashes(&manifest) {
        if !seen.insert(hex.clone()) {
            continue;
        }
        if cache.contains(&hex) {
            reused += 1;
            continue;
        }
        let loc = locations.get(&hex).with_context(|| {
            format!("chunk {hex} referenced by manifest but absent from chunk-map")
        })?;
        missing.push(loc.clone());
    }
    let total_wire: u64 = missing.iter().map(|m| m.len_stored as u64).sum();

    // 3. Concurrent range fetch.
    let stats = fetch_missing_parallel(source, &missing, &cache, opts, total_wire)?;

    // 4. Reconstruct from cache.
    reconstruct::reconstruct(&manifest, &cache, output)?;

    Ok(FetchStats {
        reused,
        logical_bytes: reconstruct::logical_bytes(&manifest),
        ..stats
    })
}

fn fetch_missing_parallel(
    source: &StaticSource,
    missing: &[ChunkMapEntry],
    cache: &ChunkCache,
    opts: &FetchOptions,
    total_wire: u64,
) -> Result<FetchStats> {
    if missing.is_empty() {
        if let Some(p) = opts.progress {
            p(0, 0);
        }
        return Ok(FetchStats::default());
    }
    let workers = opts.connections.max(1).min(missing.len());
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let first_error: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let wire = AtomicUsize::new(0);
    let raw = AtomicUsize::new(0);
    let fetched = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if failed.load(Ordering::Relaxed) {
                    return;
                }
                if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    let mut g = first_error.lock().unwrap();
                    if g.is_none() {
                        *g = Some(anyhow::Error::new(Cancelled));
                    }
                    failed.store(true, Ordering::Relaxed);
                    return;
                }
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= missing.len() {
                    return;
                }
                match fetch_one(source, &missing[idx], cache) {
                    Ok((raw_len, wire_len)) => {
                        wire.fetch_add(wire_len, Ordering::Relaxed);
                        raw.fetch_add(raw_len, Ordering::Relaxed);
                        fetched.fetch_add(1, Ordering::Relaxed);
                        if let Some(p) = opts.progress {
                            p(wire.load(Ordering::Relaxed) as u64, total_wire);
                        }
                    }
                    Err(e) => {
                        let mut g = first_error.lock().unwrap();
                        if g.is_none() {
                            *g = Some(e);
                        }
                        failed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            });
        }
    });

    if let Some(e) = first_error.into_inner().unwrap() {
        return Err(e);
    }
    Ok(FetchStats {
        wire_bytes: wire.load(Ordering::Relaxed) as u64,
        raw_bytes: raw.load(Ordering::Relaxed) as u64,
        fetched: fetched.load(Ordering::Relaxed) as u64,
        ..FetchStats::default()
    })
}

fn fetch_one(
    source: &StaticSource,
    entry: &ChunkMapEntry,
    cache: &ChunkCache,
) -> Result<(usize, usize)> {
    let hash: ChunkHash =
        from_hex(&entry.hash).with_context(|| format!("bad hash {} in chunk-map", entry.hash))?;
    let wire = source.get_range(&entry.pack, entry.pack_offset_abs, entry.len_stored as u64)?;
    let wire_len = wire.len();
    let mut raw = if entry.flags & CHUNK_FLAG_ZSTD != 0 {
        zstd::bulk::decompress(&wire, entry.len_raw as usize)
            .map_err(|e| anyhow::anyhow!("decompressing chunk {}: {e}", entry.hash))?
    } else {
        wire
    };
    if entry.flags & CHUNK_FLAG_BG4 != 0 {
        raw = bg4_ungroup(&raw);
    }
    if raw.len() != entry.len_raw as usize || hash_chunk(&raw) != hash {
        bail!(
            "CAVS-E-CHUNK-HASH-MISMATCH: chunk {} failed verification",
            entry.hash
        );
    }
    let raw_len = raw.len();
    cache.put(&hash, &raw)?;
    Ok((raw_len, wire_len))
}

/// Every unique chunk hash the manifest references (init + segment chunks).
fn manifest_chunk_hashes(manifest: &cavs_proto::Manifest) -> Vec<String> {
    let mut set = std::collections::HashSet::new();
    for t in &manifest.tracks {
        for c in &t.init_chunks {
            set.insert(c.hash.clone());
        }
    }
    for s in &manifest.segments {
        for c in &s.chunks {
            set.insert(c.hash.clone());
        }
    }
    set.into_iter().collect()
}

/// Enforce the manifest's Ed25519 content signature against a trusted key.
fn verify_signature(manifest: &cavs_proto::Manifest, trusted_hex: &str) -> Result<()> {
    use ed25519_dalek::Verifier;
    let sig_hex = manifest
        .signature
        .as_deref()
        .context("asset is not signed but a pubkey was given")?;
    let signer_hex = manifest
        .signer_pubkey
        .as_deref()
        .context("asset signature has no public key")?;
    if !signer_hex.eq_ignore_ascii_case(trusted_hex) {
        bail!("asset is signed by an untrusted key {signer_hex}");
    }
    let leaves: Vec<ChunkHash> = manifest
        .chunk_table
        .iter()
        .map(|h| from_hex(h).context("bad hash in chunk_table"))
        .collect::<Result<_>>()?;
    let root = cavs_hash::merkle_root(&leaves);
    if !manifest
        .merkle_root
        .eq_ignore_ascii_case(&cavs_hash::to_hex(&root))
    {
        bail!("manifest merkle_root does not match its chunk_table");
    }
    let pk: [u8; 32] = decode_hex(signer_hex, 32)?.try_into().unwrap();
    let sig: [u8; 64] = decode_hex(sig_hex, 64)?.try_into().unwrap();
    let key = ed25519_dalek::VerifyingKey::from_bytes(&pk).context("invalid signer key")?;
    let message = cavs_hash::content_signature_message(&root, leaves.len() as u64);
    key.verify(&message, &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| anyhow::anyhow!("content signature is INVALID"))?;
    // Every referenced chunk must be covered by the signed table.
    let table: std::collections::HashSet<&str> =
        manifest.chunk_table.iter().map(|s| s.as_str()).collect();
    for h in manifest_chunk_hashes(manifest) {
        if !table.contains(h.as_str()) {
            bail!("chunk {h} referenced but not covered by the signed table");
        }
    }
    Ok(())
}

fn decode_hex(s: &str, len: usize) -> Result<Vec<u8>> {
    if s.len() != len * 2 {
        bail!("expected {} hex chars, got {}", len * 2, s.len());
    }
    (0..len)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("bad hex"))
        .collect()
}
