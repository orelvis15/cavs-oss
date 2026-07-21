//! Metadata resolution for static trees (Round 3A).
//!
//! Historically every fetched asset cost two metadata round-trips
//! (`manifest.json` + `chunk-map.json`). On a WAN a many-object clone spends
//! most of its time in those serialized round-trips, not in payload. This
//! module collapses them:
//!
//! - **Session meta-packs**: the publisher (store finalize) writes one
//!   `meta/packs/<id>.cmeta` per push containing manifest + chunk-map for
//!   every object of that push, plus a `meta/index.json` mapping oid → pack.
//!   Resolving one oid downloads its whole pack, prefetching every sibling
//!   object of the same push — a clone resolves hundreds of objects in a
//!   handful of requests.
//! - **L1 cache**: resolved metadata is kept in-process for the session.
//! - **L2 cache**: resolved metadata persists on disk next to the chunk
//!   cache, validated against the remote's oid → pack mapping so a repack
//!   or re-push never serves stale locations.
//! - **Singleflight**: concurrent resolves of the same oid (SDK embedders)
//!   share one network fetch.
//! - **Negative cache**: a remote without `meta/index.json` (an older
//!   export) is probed once, then falls back to per-asset requests for a
//!   short TTL instead of re-probing per object.

use crate::{ChunkMapEntry, ChunkMapFile, StaticSource};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a missing `meta/index.json` is remembered before re-probing.
const NEGATIVE_TTL: Duration = Duration::from_secs(5);

/// Hard cap on a decompressed meta-pack (zstd-bomb guard).
const MAX_META_PACK_BYTES: usize = 256 * 1024 * 1024;

/// One object's resolved metadata: the reconstruction manifest and the
/// physical location of every chunk it references.
pub struct ResolvedMeta {
    pub(crate) manifest: cavs_proto::Manifest,
    pub(crate) chunks: Vec<ChunkMapEntry>,
}

/// `meta/index.json`: which meta-pack holds each oid's metadata.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetaIndexFile {
    pub version: u32,
    #[serde(default)]
    pub generation: u64,
    pub packs: Vec<MetaIndexPack>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetaIndexPack {
    pub id: String,
    pub oids: Vec<String>,
}

/// `meta/packs/<id>.cmeta` (zstd-compressed JSON): every object of one
/// publish session.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetaPackFile {
    pub version: u32,
    pub objects: Vec<MetaPackObject>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetaPackObject {
    pub oid: String,
    pub manifest: cavs_proto::Manifest,
    /// v1 locations: one verbose entry per chunk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<ChunkMapEntry>,
    /// v2 locations: runs of physically contiguous chunks — pack + start
    /// offset stated once, per-chunk offsets implicit. Preferred when
    /// present; `chunks` is the dual-read fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<MetaRun>,
}

/// One run of physically contiguous chunks inside a pack (chunk-map v2).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetaRun {
    pub pack: String,
    pub start_abs: u64,
    pub hashes: Vec<String>,
    pub lens_raw: Vec<u32>,
    pub lens_stored: Vec<u32>,
    /// A single integer when uniform across the run, else one per chunk.
    #[serde(default)]
    pub flags: RunFlags,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub(crate) enum RunFlags {
    #[default]
    None,
    Uniform(u32),
    PerChunk(Vec<u32>),
}

impl MetaPackObject {
    /// The object's chunk locations, whichever encoding it carries.
    fn into_entries(self) -> (String, cavs_proto::Manifest, Vec<ChunkMapEntry>) {
        if self.runs.is_empty() {
            return (self.oid, self.manifest, self.chunks);
        }
        let mut entries = Vec::new();
        for run in self.runs {
            let mut offset = run.start_abs;
            for (i, hash) in run.hashes.into_iter().enumerate() {
                let len_stored = run.lens_stored.get(i).copied().unwrap_or(0);
                let flags = match &run.flags {
                    RunFlags::None => 0,
                    RunFlags::Uniform(f) => *f,
                    RunFlags::PerChunk(v) => v.get(i).copied().unwrap_or(0),
                };
                entries.push(ChunkMapEntry {
                    hash,
                    len_raw: run.lens_raw.get(i).copied().unwrap_or(0),
                    len_stored,
                    flags,
                    pack: run.pack.clone(),
                    pack_offset_abs: offset,
                });
                offset += len_stored as u64;
            }
        }
        (self.oid, self.manifest, entries)
    }
}

/// L2 disk entry: the resolved metadata plus the meta-pack it came from
/// (`src`), so it can be revalidated against the remote's current mapping.
#[derive(Serialize, Deserialize)]
struct L2Entry {
    src: String,
    manifest: cavs_proto::Manifest,
    chunks: Vec<ChunkMapEntry>,
}

/// Counters for the metadata path (all monotonic within a resolver's life).
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct MetaStats {
    /// Metadata HTTP/file requests issued (index + packs + fallbacks).
    pub requests: u64,
    /// Resolves served from the in-process L1 cache.
    pub l1_hits: u64,
    /// Resolves served from the on-disk L2 cache.
    pub l2_hits: u64,
    /// Meta-packs downloaded.
    pub pack_fetches: u64,
    /// Objects prefetched into L1/L2 as pack siblings (batch fill).
    pub prefetched: u64,
    /// Resolves that fell back to per-asset manifest + chunk-map requests.
    pub fallback_singles: u64,
    /// Probes skipped because the remote's missing meta index is
    /// negative-cached.
    pub negative_hits: u64,
    /// Resolves that piggybacked on another in-flight resolve.
    pub singleflight_shared: u64,
    /// Wall time spent resolving metadata, in milliseconds.
    pub resolve_ms: u64,
}

/// Mutable resolver state (behind one short-lived lock; network work happens
/// under `fetch_lock` instead so cache hits never wait on the wire).
#[derive(Default)]
struct MetaState {
    l1: HashMap<String, Arc<ResolvedMeta>>,
    /// oid → meta-pack id, from `meta/index.json` (newest pack wins).
    index: Option<HashMap<String, String>>,
    index_missing_until: Option<Instant>,
    fetched_packs: HashSet<String>,
    stats: MetaStats,
}

/// Session-scoped metadata resolver: one per remote, shared across every
/// object fetched from it.
pub struct MetadataResolver {
    state: Mutex<MetaState>,
    /// Serializes miss-path network work: the second caller of a concurrent
    /// miss blocks here and finds L1 populated on wake (singleflight).
    fetch_lock: Mutex<()>,
    l2_root: PathBuf,
    resolve_ms: AtomicU64,
}

impl MetadataResolver {
    /// `cache_dir` is the chunk-cache root; L2 metadata lives under
    /// `<cache_dir>/meta`.
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            state: Mutex::new(MetaState::default()),
            fetch_lock: Mutex::new(()),
            l2_root: cache_dir.join("meta"),
            resolve_ms: AtomicU64::new(0),
        }
    }

    /// A snapshot of the resolver's counters.
    pub fn stats(&self) -> MetaStats {
        let mut s = self.state.lock().unwrap().stats;
        s.resolve_ms = self.resolve_ms.load(Ordering::Relaxed);
        s
    }

    /// Resolve one oid's metadata, preferring caches and batch meta-packs
    /// over per-asset requests.
    pub(crate) fn resolve(&self, source: &StaticSource, oid: &str) -> Result<Arc<ResolvedMeta>> {
        let t0 = Instant::now();
        let out = self.resolve_inner(source, oid);
        self.resolve_ms
            .fetch_add(t0.elapsed().as_millis() as u64, Ordering::Relaxed);
        out
    }

    fn resolve_inner(&self, source: &StaticSource, oid: &str) -> Result<Arc<ResolvedMeta>> {
        {
            let mut st = self.state.lock().unwrap();
            if let Some(m) = st.l1.get(oid).cloned() {
                st.stats.l1_hits += 1;
                return Ok(m);
            }
        }

        // Miss path. One resolver at a time does network work; whoever
        // waited re-checks L1 first (their answer may have been prefetched
        // or fetched by the leader).
        let _leader = self.fetch_lock.lock().unwrap();
        {
            let mut st = self.state.lock().unwrap();
            if let Some(m) = st.l1.get(oid).cloned() {
                st.stats.singleflight_shared += 1;
                return Ok(m);
            }
        }

        // 1. Make sure we know the remote's oid → pack mapping (one probe
        //    per session; a missing index is negative-cached briefly).
        let mapped_pack = self.ensure_index(source)?.and_then(|idx| idx.get(oid).cloned());

        // 2. L2 disk cache, validated against the mapping so a re-pushed or
        //    repacked object never resolves to stale chunk locations.
        if let Some(pack_id) = &mapped_pack {
            if let Some(entry) = self.l2_get(oid) {
                if entry.src == *pack_id {
                    let meta = Arc::new(ResolvedMeta {
                        manifest: entry.manifest,
                        chunks: entry.chunks,
                    });
                    let mut st = self.state.lock().unwrap();
                    st.stats.l2_hits += 1;
                    st.l1.insert(oid.to_string(), meta.clone());
                    return Ok(meta);
                }
            }
        }

        // 3. Meta-pack route: fetch the pack holding this oid and prefetch
        //    every sibling object it carries.
        if let Some(pack_id) = &mapped_pack {
            let already = self
                .state
                .lock()
                .unwrap()
                .fetched_packs
                .contains(pack_id);
            if !already {
                match self.fetch_meta_pack(source, pack_id, oid) {
                    Ok(Some(meta)) => return Ok(meta),
                    Ok(None) => {} // pack didn't carry the oid: fall through
                    Err(e) => {
                        eprintln!(
                            "[cavs-fetch] meta-pack {pack_id} unusable ({e:#}); \
                             falling back to per-asset metadata"
                        );
                    }
                }
            }
        }

        // 4. Fallback: classic per-asset manifest + chunk-map requests.
        self.resolve_single(source, oid)
    }

    /// Load `meta/index.json` once per session. Returns the oid → pack map,
    /// or `None` when the remote has no meta index (older export).
    fn ensure_index(&self, source: &StaticSource) -> Result<Option<HashMap<String, String>>> {
        {
            let mut st = self.state.lock().unwrap();
            if let Some(idx) = &st.index {
                return Ok(Some(idx.clone()));
            }
            if let Some(until) = st.index_missing_until {
                if Instant::now() < until {
                    st.stats.negative_hits += 1;
                    return Ok(None);
                }
                st.index_missing_until = None;
            }
        }
        let fetched = source.get_all_opt("meta/index.json");
        let mut st = self.state.lock().unwrap();
        st.stats.requests += 1;
        match fetched {
            Ok(Some(bytes)) => match serde_json::from_slice::<MetaIndexFile>(&bytes) {
                Ok(file) if file.version == 1 => {
                    let mut map = HashMap::new();
                    // Later packs win: a re-pushed oid resolves to its
                    // newest metadata.
                    for pack in &file.packs {
                        for oid in &pack.oids {
                            map.insert(oid.clone(), pack.id.clone());
                        }
                    }
                    st.index = Some(map.clone());
                    Ok(Some(map))
                }
                Ok(file) => {
                    eprintln!(
                        "[cavs-fetch] meta/index.json version {} not supported; \
                         using per-asset metadata",
                        file.version
                    );
                    st.index_missing_until = Some(Instant::now() + NEGATIVE_TTL);
                    Ok(None)
                }
                Err(e) => {
                    eprintln!("[cavs-fetch] meta/index.json unreadable ({e}); ignoring");
                    st.index_missing_until = Some(Instant::now() + NEGATIVE_TTL);
                    Ok(None)
                }
            },
            Ok(None) => {
                st.index_missing_until = Some(Instant::now() + NEGATIVE_TTL);
                Ok(None)
            }
            // A transport error is not proof of absence: report it so a
            // flaky network doesn't silently degrade to 2N requests.
            Err(e) => Err(e.context("fetching meta/index.json")),
        }
    }

    /// Download and ingest one meta-pack; every object it carries lands in
    /// L1 + L2. Returns the requested oid's metadata if the pack held it.
    fn fetch_meta_pack(
        &self,
        source: &StaticSource,
        pack_id: &str,
        want_oid: &str,
    ) -> Result<Option<Arc<ResolvedMeta>>> {
        let rel = format!("meta/packs/{pack_id}.cmeta");
        let compressed = {
            let mut st = self.state.lock().unwrap();
            st.stats.requests += 1;
            st.stats.pack_fetches += 1;
            st.fetched_packs.insert(pack_id.to_string());
            drop(st);
            source
                .get_all(&rel)
                .with_context(|| format!("fetching {rel}"))?
        };
        let raw = zstd::bulk::decompress(&compressed, MAX_META_PACK_BYTES)
            .with_context(|| format!("decompressing {rel}"))?;
        let pack: MetaPackFile =
            serde_json::from_slice(&raw).with_context(|| format!("parsing {rel}"))?;
        if pack.version != 1 {
            anyhow::bail!("{rel}: unsupported meta-pack version {}", pack.version);
        }

        let mut wanted = None;
        let mut st = self.state.lock().unwrap();
        for obj in pack.objects {
            let (oid, manifest, chunks) = obj.into_entries();
            let l2 = L2Entry {
                src: pack_id.to_string(),
                manifest,
                chunks,
            };
            self.l2_put(&oid, &l2);
            let meta = Arc::new(ResolvedMeta {
                manifest: l2.manifest,
                chunks: l2.chunks,
            });
            if oid == want_oid {
                wanted = Some(meta.clone());
            } else {
                st.stats.prefetched += 1;
            }
            st.l1.insert(oid, meta);
        }
        Ok(wanted)
    }

    /// The pre-Round-3 path: two per-asset requests. Also the fallback for
    /// remotes without meta-packs and for oids missing from the index.
    fn resolve_single(&self, source: &StaticSource, oid: &str) -> Result<Arc<ResolvedMeta>> {
        let manifest_bytes = source
            .get_all(&format!("assets/{oid}/manifest.json"))
            .with_context(|| format!("asset {oid}: no manifest.json in the static tree"))?;
        let manifest: cavs_proto::Manifest =
            serde_json::from_slice(&manifest_bytes).context("parsing manifest.json")?;
        let map_bytes = source
            .get_all(&format!("assets/{oid}/chunk-map.json"))
            .with_context(|| format!("asset {oid}: no chunk-map.json in the static tree"))?;
        let map: ChunkMapFile =
            serde_json::from_slice(&map_bytes).context("parsing chunk-map.json")?;
        let meta = Arc::new(ResolvedMeta {
            manifest,
            chunks: map.chunks,
        });
        let mut st = self.state.lock().unwrap();
        st.stats.requests += 2;
        st.stats.fallback_singles += 1;
        st.l1.insert(oid.to_string(), meta.clone());
        Ok(meta)
    }

    fn l2_path(&self, oid: &str) -> Option<PathBuf> {
        // Never let an oid traverse the cache dir: path separators and dots
        // are rejected outright (LFS oids are plain hex anyway).
        if oid.len() < 2
            || !oid
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
        Some(self.l2_root.join(&oid[..2]).join(format!("{oid}.meta.zst")))
    }

    fn l2_get(&self, oid: &str) -> Option<L2Entry> {
        let path = self.l2_path(oid)?;
        let compressed = std::fs::read(&path).ok()?;
        let raw = zstd::bulk::decompress(&compressed, MAX_META_PACK_BYTES).ok()?;
        match serde_json::from_slice(&raw) {
            Ok(entry) => Some(entry),
            Err(_) => {
                let _ = std::fs::remove_file(&path); // corrupt entries self-heal
                None
            }
        }
    }

    fn l2_put(&self, oid: &str, entry: &L2Entry) {
        let Some(path) = self.l2_path(oid) else {
            return;
        };
        // Best-effort: an unwritable L2 must never fail a fetch.
        let Ok(raw) = serde_json::to_vec(entry) else {
            return;
        };
        let Ok(compressed) = zstd::bulk::compress(&raw, 3) else {
            return;
        };
        if std::fs::create_dir_all(path.parent().unwrap()).is_err() {
            return;
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &compressed).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StaticSource;

    fn manifest_for(oid: &str) -> cavs_proto::Manifest {
        cavs_proto::Manifest {
            asset: oid.to_string(),
            asset_uuid: String::new(),
            tracks: vec![],
            segments: vec![],
            dict: vec![],
            chunk_table: vec![],
            merkle_root: String::new(),
            signature: None,
            signer_pubkey: None,
            meta: vec![],
        }
    }

    fn chunk_entry(hash: &str) -> ChunkMapEntry {
        ChunkMapEntry {
            hash: hash.to_string(),
            len_raw: 4,
            len_stored: 4,
            flags: 0,
            pack: "chunks/packs/aa/pack.cavspack".to_string(),
            pack_offset_abs: 16,
        }
    }

    /// Build a static tree with a meta index + one pack holding `oids`.
    fn tree_with_meta_pack(root: &Path, pack_id: &str, oids: &[&str]) {
        let pack = MetaPackFile {
            version: 1,
            objects: oids
                .iter()
                .map(|oid| MetaPackObject {
                    oid: oid.to_string(),
                    manifest: manifest_for(oid),
                    chunks: vec![chunk_entry(&format!("c-{oid}"))],
                    runs: vec![],
                })
                .collect(),
        };
        let raw = serde_json::to_vec(&pack).unwrap();
        let compressed = zstd::bulk::compress(&raw, 3).unwrap();
        std::fs::create_dir_all(root.join("meta/packs")).unwrap();
        std::fs::write(root.join(format!("meta/packs/{pack_id}.cmeta")), compressed).unwrap();
        let index = MetaIndexFile {
            version: 1,
            generation: 1,
            packs: vec![MetaIndexPack {
                id: pack_id.to_string(),
                oids: oids.iter().map(|s| s.to_string()).collect(),
            }],
        };
        std::fs::write(
            root.join("meta/index.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();
    }

    fn write_single_asset(root: &Path, oid: &str) {
        let dir = root.join("assets").join(oid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest_for(oid)).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("chunk-map.json"),
            serde_json::to_vec(&serde_json::json!({
                "asset": oid,
                "chunks": [chunk_entry(&format!("c-{oid}"))],
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn run_encoded_meta_pack_expands_to_per_chunk_entries() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        // One object whose locations travel as a single 3-chunk run.
        let pack = MetaPackFile {
            version: 1,
            objects: vec![MetaPackObject {
                oid: "oid-r".into(),
                manifest: manifest_for("oid-r"),
                chunks: vec![],
                runs: vec![MetaRun {
                    pack: "chunks/packs/aa/p.cavspack".into(),
                    start_abs: 16,
                    hashes: vec!["h0".into(), "h1".into(), "h2".into()],
                    lens_raw: vec![100, 200, 300],
                    lens_stored: vec![50, 60, 70],
                    flags: RunFlags::Uniform(3),
                }],
            }],
        };
        let raw = serde_json::to_vec(&pack).unwrap();
        std::fs::create_dir_all(remote.path().join("meta/packs")).unwrap();
        std::fs::write(
            remote.path().join("meta/packs/runpack.cmeta"),
            zstd::bulk::compress(&raw, 3).unwrap(),
        )
        .unwrap();
        let index = MetaIndexFile {
            version: 1,
            generation: 1,
            packs: vec![MetaIndexPack {
                id: "runpack".into(),
                oids: vec!["oid-r".into()],
            }],
        };
        std::fs::write(
            remote.path().join("meta/index.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();

        let source = StaticSource::new(remote.path().to_str().unwrap());
        let resolver = MetadataResolver::new(cache.path());
        let m = resolver.resolve(&source, "oid-r").unwrap();
        assert_eq!(m.chunks.len(), 3);
        // Offsets are implicit: cumulative stored lengths from start_abs.
        assert_eq!(m.chunks[0].pack_offset_abs, 16);
        assert_eq!(m.chunks[1].pack_offset_abs, 66);
        assert_eq!(m.chunks[2].pack_offset_abs, 126);
        assert!(m.chunks.iter().all(|c| c.flags == 3));
        assert_eq!(m.chunks[2].len_raw, 300);
        assert_eq!(m.chunks[2].pack, "chunks/packs/aa/p.cavspack");
    }

    #[test]
    fn meta_pack_resolves_and_prefetches_siblings() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        tree_with_meta_pack(remote.path(), "deadbeef", &["oid-a", "oid-b", "oid-c"]);
        let source = StaticSource::new(remote.path().to_str().unwrap());
        let resolver = MetadataResolver::new(cache.path());

        let a = resolver.resolve(&source, "oid-a").unwrap();
        assert_eq!(a.manifest.asset, "oid-a");
        let s = resolver.stats();
        assert_eq!(s.requests, 2, "one index + one pack request");
        assert_eq!(s.prefetched, 2, "siblings b and c prefetched");

        // Siblings are L1 hits: zero further requests.
        resolver.resolve(&source, "oid-b").unwrap();
        resolver.resolve(&source, "oid-c").unwrap();
        let s = resolver.stats();
        assert_eq!(s.requests, 2);
        assert_eq!(s.l1_hits, 2);
    }

    #[test]
    fn l2_survives_a_new_resolver_and_validates_pack_id() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        tree_with_meta_pack(remote.path(), "pack1", &["oid-a"]);
        let source = StaticSource::new(remote.path().to_str().unwrap());

        MetadataResolver::new(cache.path())
            .resolve(&source, "oid-a")
            .unwrap();

        // Second session: index request + L2 hit, no pack re-download.
        let resolver = MetadataResolver::new(cache.path());
        resolver.resolve(&source, "oid-a").unwrap();
        let s = resolver.stats();
        assert_eq!((s.l2_hits, s.pack_fetches, s.requests), (1, 0, 1));

        // The remote re-published oid-a in a new pack: stale L2 must NOT be
        // served; the new pack is fetched instead.
        tree_with_meta_pack(remote.path(), "pack2", &["oid-a"]);
        let resolver = MetadataResolver::new(cache.path());
        resolver.resolve(&source, "oid-a").unwrap();
        let s = resolver.stats();
        assert_eq!((s.l2_hits, s.pack_fetches), (0, 1), "stale L2 rejected");
    }

    #[test]
    fn remote_without_meta_index_falls_back_and_negative_caches() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        write_single_asset(remote.path(), "oid-a");
        write_single_asset(remote.path(), "oid-b");
        let source = StaticSource::new(remote.path().to_str().unwrap());
        let resolver = MetadataResolver::new(cache.path());

        resolver.resolve(&source, "oid-a").unwrap();
        resolver.resolve(&source, "oid-b").unwrap();
        let s = resolver.stats();
        // 1 failed index probe + 2×2 per-asset requests; second resolve
        // hits the negative cache instead of re-probing.
        assert_eq!(s.requests, 5);
        assert_eq!(s.fallback_singles, 2);
        assert_eq!(s.negative_hits, 1);
    }

    #[test]
    fn oid_missing_from_index_falls_back_to_single_requests() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        tree_with_meta_pack(remote.path(), "pack1", &["oid-a"]);
        write_single_asset(remote.path(), "oid-old");
        let source = StaticSource::new(remote.path().to_str().unwrap());
        let resolver = MetadataResolver::new(cache.path());

        let m = resolver.resolve(&source, "oid-old").unwrap();
        assert_eq!(m.manifest.asset, "oid-old");
        assert_eq!(resolver.stats().fallback_singles, 1);
    }

    #[test]
    fn corrupt_meta_pack_falls_back_instead_of_failing() {
        let remote = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        tree_with_meta_pack(remote.path(), "pack1", &["oid-a"]);
        write_single_asset(remote.path(), "oid-a");
        std::fs::write(remote.path().join("meta/packs/pack1.cmeta"), b"garbage").unwrap();
        let source = StaticSource::new(remote.path().to_str().unwrap());
        let resolver = MetadataResolver::new(cache.path());

        let m = resolver.resolve(&source, "oid-a").unwrap();
        assert_eq!(m.manifest.asset, "oid-a");
        assert_eq!(resolver.stats().fallback_singles, 1);
    }
}
