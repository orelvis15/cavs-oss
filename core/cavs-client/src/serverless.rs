//! Serverless / CDN-only fetch (v1.4.0).
//!
//! `cavs store export --static-plans` writes a fully static tree — immutable
//! `.cavspack` files plus, per asset, a `manifest.json` (reconstruction
//! structure) and a `chunk-map.json` (each chunk's pack + absolute byte
//! range). This module fetches an asset straight from that tree with **no
//! `cavs-server`**: it plans the missing set locally from the manifest and
//! the cache, HTTP-Range-GETs (or, for a local directory, slice-reads) only
//! the chunks it lacks — concurrently — verifies each by BLAKE3, and
//! reconstructs from the cache. The tree can live on S3, R2, GitHub Pages,
//! nginx, or a local folder; any host that serves bytes and honours Range.

use crate::cache::ChunkCache;
use crate::retry;
use anyhow::{bail, Context, Result};
use cavs_hash::{from_hex, hash_chunk, ChunkHash};
use cavs_proto::errors::ErrorCode;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// zstd chunk flag in the store ledger / chunk-map (mirrors
/// `cavs_format::CHUNK_FLAG_ZSTD`).
const CHUNK_FLAG_ZSTD: u32 = 1;

/// BG4 pretransform chunk flag (mirrors `cavs_format::CHUNK_FLAG_BG4`).
const CHUNK_FLAG_BG4: u32 = 1 << 1;

/// Inverse of the BG4 byte-grouping pretransform (mirrors
/// `cavs_format::bg4_ungroup`).
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
    /// Absolute byte offset of the stored chunk within the pack file.
    pack_offset_abs: u64,
}

/// Where the static tree lives: a base URL or a local directory.
pub enum StaticSource {
    Http { base: String, agent: ureq::Agent },
    Dir(PathBuf),
}

impl StaticSource {
    /// Interpret `base` as an http(s) URL or a filesystem path.
    pub fn parse(base: &str, agent: ureq::Agent) -> Self {
        if base.starts_with("http://") || base.starts_with("https://") {
            StaticSource::Http {
                base: base.trim_end_matches('/').to_string(),
                agent,
            }
        } else {
            StaticSource::Dir(PathBuf::from(base))
        }
    }

    /// Fetch a whole small object (manifest / chunk-map).
    fn get_all(&self, rel: &str) -> Result<Vec<u8>> {
        match self {
            StaticSource::Http { base, agent } => {
                let url = format!("{base}/{rel}");
                let resp = retry::with_retry(&format!("GET {url}"), || agent.get(&url).call())?;
                let mut out = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut out)
                    .with_context(|| format!("reading {url}"))?;
                Ok(out)
            }
            StaticSource::Dir(root) => {
                let path = root.join(rel);
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
            }
        }
    }

    /// Fetch a byte range `[offset, offset+len)` of a pack object.
    fn get_range(&self, rel: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        match self {
            StaticSource::Http { base, agent } => {
                let url = format!("{base}/{rel}");
                let end = offset + len - 1;
                let range = format!("bytes={offset}-{end}");
                let resp = retry::with_retry(&format!("GET {url} [{range}]"), || {
                    agent.get(&url).set("range", &range).call()
                })?;
                let mut out = Vec::with_capacity(len as usize);
                resp.into_reader()
                    .read_to_end(&mut out)
                    .with_context(|| format!("reading range of {url}"))?;
                // A static host that ignores Range returns the whole object
                // (200); slice out what we asked for so we still work.
                if out.len() as u64 > len {
                    let start = offset as usize;
                    let stop = start + len as usize;
                    if stop <= out.len() {
                        return Ok(out[start..stop].to_vec());
                    }
                }
                Ok(out)
            }
            StaticSource::Dir(root) => {
                use std::io::{Seek, SeekFrom};
                let path = root.join(rel);
                let mut f = std::fs::File::open(&path)
                    .with_context(|| format!("opening {}", path.display()))?;
                f.seek(SeekFrom::Start(offset))?;
                let mut out = vec![0u8; len as usize];
                f.read_exact(&mut out)
                    .with_context(|| format!("reading range of {}", path.display()))?;
                Ok(out)
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StaticStats {
    pub wire_bytes: u64,
    pub raw_bytes: u64,
    pub fetched: u64,
    pub reused: u64,
    pub logical_bytes: u64,
}

/// Fetch `asset` from the static tree at `source` into `output`, using
/// `cache` and up to `connections` concurrent range requests. Returns the
/// reconstructed primary paths and egress stats.
pub fn fetch_static(
    source: &StaticSource,
    asset: &str,
    output: &Path,
    cache: &ChunkCache,
    connections: usize,
    pubkey: Option<&str>,
) -> Result<(Vec<PathBuf>, StaticStats)> {
    // 1. Manifest (JSON in the static tree) + chunk-map.
    let manifest_bytes = source
        .get_all(&format!("assets/{asset}/manifest.json"))
        .with_context(|| format!("asset {asset}: no manifest.json in the static tree"))?;
    let loaded = crate::decode_manifest(&manifest_bytes)?;
    let manifest = loaded.manifest;

    if let Some(pk) = pubkey {
        crate::verify_manifest_signature(&manifest, pk)
            .map_err(|e| anyhow::anyhow!(ErrorCode::SignatureInvalid.msg(format!("{e:#}"))))?;
        eprintln!("[static] content signature OK");
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

    // 2. Missing set: unique manifest chunks minus what the cache holds.
    let mut seen = std::collections::HashSet::new();
    let mut missing: Vec<ChunkMapEntry> = Vec::new();
    let mut reused = 0u64;
    for hex in crate::manifest_chunk_hashes(&manifest) {
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
    eprintln!(
        "[static] {} missing / {} already cached, {} connection(s)",
        missing.len(),
        reused,
        connections
    );

    // 3. Concurrent range fetch: verify each chunk, land it in the cache.
    let stats = fetch_missing_parallel(source, &missing, cache, connections.max(1))?;

    // 4. Reconstruct purely from the (now complete) cache.
    let primaries = crate::reconstruct_streaming(&manifest, cache, output)?;

    let logical = crate::manifest_logical_bytes(&manifest);
    println!(
        "static  : {asset} -> {} ({} file(s))",
        output.display(),
        manifest.tracks.len()
    );
    let out = StaticStats {
        reused,
        logical_bytes: logical,
        ..stats
    };
    println!(
        "egress  : {} wire ({} raw, {} chunks) / {} reused from cache",
        crate::human_bytes(out.wire_bytes),
        crate::human_bytes(out.raw_bytes),
        out.fetched,
        out.reused
    );
    println!(
        "logical : {}  -> saved {:.2}% of egress",
        crate::human_bytes(out.logical_bytes),
        if out.logical_bytes == 0 {
            0.0
        } else {
            (out.logical_bytes.saturating_sub(out.wire_bytes)) as f64 * 100.0
                / out.logical_bytes as f64
        }
    );
    Ok((primaries, out))
}

fn fetch_missing_parallel(
    source: &StaticSource,
    missing: &[ChunkMapEntry],
    cache: &ChunkCache,
    connections: usize,
) -> Result<StaticStats> {
    if missing.is_empty() {
        return Ok(StaticStats::default());
    }
    let workers = connections.min(missing.len());
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
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= missing.len() {
                    return;
                }
                match fetch_one(source, &missing[idx], cache) {
                    Ok((raw_len, wire_len)) => {
                        wire.fetch_add(wire_len, Ordering::Relaxed);
                        raw.fetch_add(raw_len, Ordering::Relaxed);
                        fetched.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        failed.store(true, Ordering::Relaxed);
                        let mut g = first_error.lock().unwrap();
                        if g.is_none() {
                            *g = Some(e);
                        }
                        return;
                    }
                }
            });
        }
    });

    if let Some(e) = first_error.into_inner().unwrap() {
        return Err(e);
    }
    Ok(StaticStats {
        wire_bytes: wire.load(Ordering::Relaxed) as u64,
        raw_bytes: raw.load(Ordering::Relaxed) as u64,
        fetched: fetched.load(Ordering::Relaxed) as u64,
        ..StaticStats::default()
    })
}

/// Range-fetch one stored chunk, decompress if flagged, verify, and cache.
/// Returns `(raw_len, wire_len)`.
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
            "{}",
            ErrorCode::ChunkHashMismatch.msg(format!(
                "chunk {} failed verification (len {} vs {})",
                entry.hash,
                raw.len(),
                entry.len_raw
            ))
        );
    }
    let raw_len = raw.len();
    cache.put(&hash, &raw)?;
    Ok((raw_len, wire_len))
}
