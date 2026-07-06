//! Server state: loaded assets, per-session have-sets, the inline/ref
//! planner and metrics.

use anyhow::{Context, Result};
use cavs_format::{
    Reader, SegmentRecord, TrackKind, TrackRecord, CHUNK_FLAG_ZSTD, SEGMENT_FLAG_RANDOM_ACCESS,
};
use cavs_hash::{from_hex, hash_chunk, to_hex, ChunkHash};
use cavs_proto::{
    AssetSummary, BatchRequest, BatchResponse, ChunkRef, DeliveryInstr, InitDelivery, Manifest,
    ManifestSegment, ManifestTrack, SegmentDelivery, SessionOpenResponse, DELIVERY_BOOTSTRAP,
    DELIVERY_CHUNKS, DELIVERY_REFERENCES, WIRE_COMPRESSION_NONE, WIRE_COMPRESSION_ZSTD,
};
use cavs_store::GlobalStore;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub type SharedState = Arc<AppState>;

/// Where an asset's chunk bytes come from: a self-contained `.cavs` file, or
/// the shared global content-addressable store (dedup across all assets).
enum ChunkSource {
    File(Mutex<Box<Reader>>),
    /// idx -> hash, resolved against the shared store.
    Store {
        store: Arc<Mutex<GlobalStore>>,
        hashes: Vec<ChunkHash>,
    },
}

impl ChunkSource {
    /// Chunk as stored (possibly zstd), for wire passthrough.
    fn read_stored(&self, idx: u32) -> std::result::Result<(Vec<u8>, u32, u32), String> {
        match self {
            ChunkSource::File(r) => r
                .lock()
                .unwrap()
                .read_chunk_stored(idx)
                .map_err(|e| format!("chunk {idx}: {e}")),
            ChunkSource::Store { store, hashes } => {
                let hash = hashes.get(idx as usize).ok_or("chunk index out of range")?;
                store
                    .lock()
                    .unwrap()
                    .read_chunk_stored(hash)
                    .map_err(|e| format!("chunk {idx}: {e}"))
            }
        }
    }

    /// Chunk decompressed and BLAKE3-verified (for HLS/direct reads).
    fn read_raw(&self, idx: u32) -> Option<Vec<u8>> {
        match self {
            ChunkSource::File(r) => r.lock().unwrap().read_chunk(idx).ok(),
            ChunkSource::Store { .. } => {
                let (stored, flags, len_raw) = self.read_stored(idx).ok()?;
                let hash = match self {
                    ChunkSource::Store { hashes, .. } => *hashes.get(idx as usize)?,
                    _ => unreachable!(),
                };
                let raw = if flags & CHUNK_FLAG_ZSTD != 0 {
                    zstd::bulk::decompress(&stored, len_raw as usize).ok()?
                } else {
                    stored
                };
                (hash_chunk(&raw) == hash).then_some(raw)
            }
        }
    }
}

/// A packed full-artifact sidecar (`<asset>.cavs.bootstrap.zst`): the whole
/// asset zstd-compressed, offered to cache-less clients when it beats the
/// chunk path (v2 dual route).
pub struct Bootstrap {
    pub path: PathBuf,
    pub size: u64,
    pub blake3_hex: String,
}

pub struct Asset {
    source: ChunkSource,
    tracks: Vec<TrackRecord>,
    segments: Vec<SegmentRecord>,
    /// chunk index -> (hash, raw length, stored length, storage flags)
    chunk_meta: Vec<(ChunkHash, u32, u32, u32)>,
    index_by_hash: HashMap<ChunkHash, u32>,
    dict: Vec<u32>,
    uuid_hex: String,
    merkle_root_hex: String,
    /// (signature hex, pubkey hex) when signed.
    signature: Option<(String, String)>,
    meta: Vec<(String, String)>,
    bootstrap: Option<Bootstrap>,
}

/// Bootstrap routing (v2 dual route): a client is "cold" below this share of
/// known chunks…
const BOOTSTRAP_HAVE_RATIO_MAX: f64 = 0.05;
/// …and the bootstrap must be at least this much cheaper than the estimated
/// chunk payload (0.98 = 2% margin, per the v2 plan).
const BOOTSTRAP_COLD_THRESHOLD: f64 = 0.98;
/// Per-instruction overhead of the CVSP wire encoding (tag + hash + lens).
const WIRE_OVERHEAD_PER_CHUNK: u64 = 42;

struct Session {
    asset: String,
    /// Chunk indices (within the asset's table) the client is known to have.
    known: HashSet<u32>,
}

#[derive(Default)]
pub struct Metrics {
    pub sessions_opened_total: AtomicU64,
    pub batches_total: AtomicU64,
    pub chunks_inline_total: AtomicU64,
    pub refs_sent_total: AtomicU64,
    pub bytes_inline_total: AtomicU64,
    pub bundle_collapses_total: AtomicU64,
    pub hls_requests_total: AtomicU64,
    pub chunk_requests_total: AtomicU64,
    pub bootstrap_sessions_total: AtomicU64,
    pub bootstraps_served_total: AtomicU64,
    pub bootstrap_bytes_total: AtomicU64,
    pub manifest_json_requests_total: AtomicU64,
    pub manifest_json_bytes_total: AtomicU64,
    pub manifest_binary_requests_total: AtomicU64,
    pub manifest_binary_bytes_total: AtomicU64,
}

pub struct AppState {
    assets: HashMap<String, Asset>,
    sessions: Mutex<HashMap<String, Session>>,
    metrics: Metrics,
    max_cold: usize,
    web_wasm: PathBuf,
}

impl AppState {
    pub fn load(paths: &[PathBuf], max_cold: usize, web_wasm: PathBuf) -> Result<Self> {
        let mut assets = HashMap::new();
        for path in paths {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "asset".to_string());
            let reader =
                Reader::open(path).with_context(|| format!("cannot open {}", path.display()))?;
            // Refuse to serve content whose embedded signature is invalid.
            let signature = match reader.verify_signature() {
                Ok(cavs_format::SignatureStatus::Valid(pk)) => {
                    let (sig, _) = reader.embedded_signature().unwrap();
                    Some((to_hex_slice(&sig), to_hex_slice(&pk)))
                }
                Ok(cavs_format::SignatureStatus::Unsigned) => None,
                Err(e) => anyhow::bail!("{}: {e}", path.display()),
            };
            let tracks = reader.tracks().to_vec();
            let segments = reader.segments().to_vec();
            let chunk_meta: Vec<(ChunkHash, u32, u32, u32)> = reader
                .chunks()
                .iter()
                .map(|c| (c.hash, c.len_raw, c.len_stored, c.flags))
                .collect();
            let index_by_hash = chunk_meta
                .iter()
                .enumerate()
                .map(|(i, (h, _, _, _))| (*h, i as u32))
                .collect();
            let dict = reader.dict().to_vec();
            let uuid_hex = reader
                .superblock()
                .asset_uuid
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let merkle_root_hex = to_hex(&reader.integrity().merkle_root);
            let meta = reader.meta().to_vec();
            let bootstrap = load_bootstrap_sidecar(path, &meta);
            assets.insert(
                name,
                Asset {
                    source: ChunkSource::File(Mutex::new(Box::new(reader))),
                    tracks,
                    segments,
                    chunk_meta,
                    index_by_hash,
                    dict,
                    uuid_hex,
                    merkle_root_hex,
                    signature,
                    meta,
                    bootstrap,
                },
            );
        }
        Ok(Self {
            assets,
            sessions: Mutex::new(HashMap::new()),
            metrics: Metrics::default(),
            max_cold,
            web_wasm,
        })
    }

    /// Serve every published asset from a shared global content-addressable
    /// store (chunks deduplicated across all assets and versions).
    pub fn load_store(
        store_dir: &std::path::Path,
        max_cold: usize,
        web_wasm: PathBuf,
    ) -> Result<Self> {
        let store = GlobalStore::open(store_dir)
            .with_context(|| format!("cannot open store {}", store_dir.display()))?;
        let shared = Arc::new(Mutex::new(store));
        let mut assets = HashMap::new();

        let names = shared.lock().unwrap().asset_names();
        for name in names {
            let record = shared.lock().unwrap().get_asset(&name)?;

            // chunk table order defines the index space.
            let mut chunk_meta = Vec::with_capacity(record.chunk_table.len());
            let mut hashes = Vec::with_capacity(record.chunk_table.len());
            let mut index_by_hash = HashMap::new();
            {
                let guard = shared.lock().unwrap();
                for (i, hex) in record.chunk_table.iter().enumerate() {
                    let hash = from_hex(hex)
                        .ok_or_else(|| anyhow::anyhow!("bad hash in {name}: {hex}"))?;
                    let info = guard
                        .chunk_info(&hash)
                        .ok_or_else(|| anyhow::anyhow!("{name} references missing chunk {hex}"))?;
                    chunk_meta.push((hash, info.len_raw, info.len_stored, info.flags));
                    hashes.push(hash);
                    index_by_hash.insert(hash, i as u32);
                }
            }
            let idx_of = |hex: &str| -> Result<u32> {
                let hash = from_hex(hex).ok_or_else(|| anyhow::anyhow!("bad hash {hex}"))?;
                index_by_hash
                    .get(&hash)
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("{name}: hash not in table {hex}"))
            };

            let mut tracks = Vec::new();
            for t in &record.tracks {
                tracks.push(TrackRecord {
                    track_id: t.track_id,
                    kind: TrackKind::from_u8(t.kind)
                        .ok_or_else(|| anyhow::anyhow!("bad track kind {}", t.kind))?,
                    flags: 0,
                    codec: t.codec.clone(),
                    name: t.name.clone(),
                    timescale: t.timescale,
                    init_chunks: t
                        .init_chunks
                        .iter()
                        .map(|h| idx_of(h))
                        .collect::<Result<_>>()?,
                });
            }
            let mut segments = Vec::new();
            for s in &record.segments {
                segments.push(SegmentRecord {
                    segment_id: s.segment_id,
                    track_id: s.track_id,
                    pts_start: s.pts_start,
                    duration: s.duration,
                    flags: if s.random_access {
                        SEGMENT_FLAG_RANDOM_ACCESS
                    } else {
                        0
                    },
                    chunks: s.chunks.iter().map(|h| idx_of(h)).collect::<Result<_>>()?,
                });
            }
            let dict = record
                .dict
                .iter()
                .map(|h| idx_of(h))
                .collect::<Result<_>>()?;
            let signature = match (&record.signature, &record.signer_pubkey) {
                (Some(sig), Some(pk)) => Some((sig.clone(), pk.clone())),
                _ => None,
            };

            assets.insert(
                name.clone(),
                Asset {
                    source: ChunkSource::Store {
                        store: shared.clone(),
                        hashes,
                    },
                    tracks,
                    segments,
                    chunk_meta,
                    index_by_hash,
                    dict,
                    uuid_hex: record.asset_uuid.clone(),
                    merkle_root_hex: record.merkle_root.clone(),
                    signature,
                    meta: record.meta.clone(),
                    // Bootstrap sidecars are not ingested into the global
                    // store yet; store-served assets use the chunk path.
                    bootstrap: None,
                },
            );
        }

        Ok(Self {
            assets,
            sessions: Mutex::new(HashMap::new()),
            metrics: Metrics::default(),
            max_cold,
            web_wasm,
        })
    }

    pub fn web_wasm_path(&self) -> &std::path::Path {
        &self.web_wasm
    }

    pub fn asset_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.assets.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn video_track_names(&self, asset: &str) -> Vec<String> {
        self.assets
            .get(asset)
            .map(|a| {
                a.tracks
                    .iter()
                    .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Audio))
                    .map(|t| t.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn summaries(&self) -> Vec<AssetSummary> {
        let mut out: Vec<AssetSummary> = self
            .assets
            .iter()
            .map(|(name, a)| AssetSummary {
                name: name.clone(),
                tracks: a.tracks.len(),
                segments: a.segments.len(),
                unique_chunks: a.chunk_meta.len() as u64,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn manifest(&self, asset_name: &str) -> Option<Manifest> {
        let asset = self.assets.get(asset_name)?;
        let chunk_ref = |idx: &u32| {
            let (hash, len, _, _) = &asset.chunk_meta[*idx as usize];
            ChunkRef {
                hash: to_hex(hash),
                len: *len,
            }
        };
        Some(Manifest {
            asset: asset_name.to_string(),
            asset_uuid: asset.uuid_hex.clone(),
            tracks: asset
                .tracks
                .iter()
                .map(|t| ManifestTrack {
                    track_id: t.track_id,
                    kind: t.kind.label().to_string(),
                    codec: t.codec.clone(),
                    name: t.name.clone(),
                    timescale: t.timescale,
                    init_chunks: t.init_chunks.iter().map(chunk_ref).collect(),
                })
                .collect(),
            segments: asset
                .segments
                .iter()
                .map(|s| ManifestSegment {
                    segment_id: s.segment_id,
                    track_id: s.track_id,
                    pts_start: s.pts_start,
                    duration: s.duration,
                    random_access: s.flags & SEGMENT_FLAG_RANDOM_ACCESS != 0,
                    chunks: s.chunks.iter().map(chunk_ref).collect(),
                })
                .collect(),
            dict: asset
                .dict
                .iter()
                .map(|i| to_hex(&asset.chunk_meta[*i as usize].0))
                .collect(),
            chunk_table: asset
                .chunk_meta
                .iter()
                .map(|(h, _, _, _)| to_hex(h))
                .collect(),
            merkle_root: asset.merkle_root_hex.clone(),
            signature: asset.signature.as_ref().map(|(sig, _)| sig.clone()),
            signer_pubkey: asset.signature.as_ref().map(|(_, pk)| pk.clone()),
            meta: asset.meta.clone(),
        })
    }

    pub fn open_session(
        &self,
        asset_name: &str,
        have: &[String],
        have_bloom: Option<&cavs_proto::BloomFilter>,
    ) -> Option<SessionOpenResponse> {
        let asset = self.assets.get(asset_name)?;
        let mut known = HashSet::new();
        if let Some(bloom) = have_bloom {
            // Bloom summary: test every chunk of the asset for membership.
            // False positives just make us send a Ref the client repairs by
            // fetching that chunk directly; never a correctness issue.
            for (hash, &idx) in &asset.index_by_hash {
                if bloom.contains(hash) {
                    known.insert(idx);
                }
            }
        } else {
            for hex in have {
                if let Some(hash) = from_hex(hex) {
                    if let Some(&idx) = asset.index_by_hash.get(&hash) {
                        known.insert(idx);
                    }
                }
            }
        }
        let known_chunks = known.len();

        // v2 dual route: estimate what the chunk path would cost this client
        // and offer the full bootstrap artifact when it is meaningfully
        // cheaper for a cold cache. Advisory — the chunk path stays valid.
        let estimated_chunk_payload: u64 = asset
            .chunk_meta
            .iter()
            .enumerate()
            .filter(|(i, _)| !known.contains(&(*i as u32)))
            .map(|(_, (_, _, len_stored, _))| *len_stored as u64 + WIRE_OVERHEAD_PER_CHUNK)
            .sum();
        let total = asset.chunk_meta.len().max(1);
        let have_ratio = known_chunks as f64 / total as f64;
        let (delivery_mode, bootstrap_size, bootstrap_blake3) = if estimated_chunk_payload == 0 {
            (DELIVERY_REFERENCES, None, None)
        } else {
            match &asset.bootstrap {
                Some(b)
                    if have_ratio < BOOTSTRAP_HAVE_RATIO_MAX
                        && (b.size as f64)
                            <= estimated_chunk_payload as f64 * BOOTSTRAP_COLD_THRESHOLD =>
                {
                    self.metrics
                        .bootstrap_sessions_total
                        .fetch_add(1, Ordering::Relaxed);
                    (DELIVERY_BOOTSTRAP, Some(b.size), Some(b.blake3_hex.clone()))
                }
                _ => (DELIVERY_CHUNKS, None, None),
            }
        };

        let session_id = uuid::Uuid::new_v4().to_string();
        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            Session {
                asset: asset_name.to_string(),
                known,
            },
        );
        self.metrics
            .sessions_opened_total
            .fetch_add(1, Ordering::Relaxed);
        Some(SessionOpenResponse {
            session_id,
            known_chunks,
            delivery_mode: Some(delivery_mode.to_string()),
            bootstrap_size,
            bootstrap_blake3,
            estimated_chunk_payload: Some(estimated_chunk_payload),
        })
    }

    /// The distinctive piece: per-session inline/ref planning.
    ///
    /// For each requested init/segment, chunks in the session's known set are
    /// sent as references; cold chunks are sent inline and recorded as known.
    /// If `max_cold` is set and a segment exceeds it, the segment collapses
    /// to a fully-inline self-sufficient bundle.
    pub fn plan_batch(&self, session_id: &str, req: &BatchRequest) -> Result<Vec<u8>, String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("unknown session {session_id}"))?;
        let asset = self
            .assets
            .get(&session.asset)
            .ok_or_else(|| "asset vanished".to_string())?;

        let mut resp = BatchResponse::default();

        for &track_id in &req.track_inits {
            let track = asset
                .tracks
                .iter()
                .find(|t| t.track_id == track_id)
                .ok_or_else(|| format!("unknown track {track_id}"))?;
            let instrs = self.plan_chunks(asset, &mut session.known, &track.init_chunks, false)?;
            resp.inits.push(InitDelivery { track_id, instrs });
        }

        for &segment_id in &req.segment_ids {
            let segment = asset
                .segments
                .iter()
                .find(|s| s.segment_id == segment_id)
                .ok_or_else(|| format!("unknown segment {segment_id}"))?;
            // Collapse policy from the design study: too many cold
            // dependencies -> ship a self-sufficient bundle.
            let cold = segment
                .chunks
                .iter()
                .filter(|c| !session.known.contains(c))
                .count();
            let collapse = self.max_cold > 0 && cold > self.max_cold;
            if collapse {
                self.metrics
                    .bundle_collapses_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            let instrs = self.plan_chunks(asset, &mut session.known, &segment.chunks, collapse)?;
            resp.segments.push(SegmentDelivery { segment_id, instrs });
        }

        self.metrics.batches_total.fetch_add(1, Ordering::Relaxed);
        Ok(resp.encode())
    }

    fn plan_chunks(
        &self,
        asset: &Asset,
        known: &mut HashSet<u32>,
        chunk_indices: &[u32],
        force_inline: bool,
    ) -> Result<Vec<DeliveryInstr>, String> {
        let mut instrs = Vec::with_capacity(chunk_indices.len());
        for &idx in chunk_indices {
            let hash = asset.chunk_meta[idx as usize].0;
            if !force_inline && known.contains(&idx) {
                self.metrics.refs_sent_total.fetch_add(1, Ordering::Relaxed);
                instrs.push(DeliveryInstr::Ref { hash });
            } else {
                // Wire passthrough: send the payload exactly as stored, so
                // zstd-compressed chunks travel compressed at zero extra CPU.
                let (payload, flags, len_raw) = asset.source.read_stored(idx)?;
                self.metrics
                    .chunks_inline_total
                    .fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .bytes_inline_total
                    .fetch_add(payload.len() as u64, Ordering::Relaxed);
                known.insert(idx);
                instrs.push(DeliveryInstr::Inline {
                    hash,
                    len_raw,
                    compression: if flags & CHUNK_FLAG_ZSTD != 0 {
                        WIRE_COMPRESSION_ZSTD
                    } else {
                        WIRE_COMPRESSION_NONE
                    },
                    payload,
                });
            }
        }
        Ok(instrs)
    }

    /// Count one manifest response in the per-format metrics.
    pub fn count_manifest_request(&self, format: &str, bytes: u64) {
        let (requests, total_bytes) = if format == "binary-v2" {
            (
                &self.metrics.manifest_binary_requests_total,
                &self.metrics.manifest_binary_bytes_total,
            )
        } else {
            (
                &self.metrics.manifest_json_requests_total,
                &self.metrics.manifest_json_bytes_total,
            )
        };
        requests.fetch_add(1, Ordering::Relaxed);
        total_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Path and size of the asset's verified bootstrap sidecar, if any.
    /// Counts the request in the bootstrap metrics.
    pub fn bootstrap_file(&self, asset_name: &str) -> Option<(PathBuf, u64)> {
        let b = self.assets.get(asset_name)?.bootstrap.as_ref()?;
        self.metrics
            .bootstraps_served_total
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .bootstrap_bytes_total
            .fetch_add(b.size, Ordering::Relaxed);
        Some((b.path.clone(), b.size))
    }

    pub fn chunk_by_hash(&self, asset_name: &str, hash_hex: &str) -> Option<Vec<u8>> {
        let asset = self.assets.get(asset_name)?;
        let idx = *asset.index_by_hash.get(&from_hex(hash_hex)?)?;
        self.metrics
            .chunk_requests_total
            .fetch_add(1, Ordering::Relaxed);
        asset.source.read_raw(idx)
    }

    /// Reconstruct HLS artifacts on the fly for direct standard playback.
    pub fn hls_file(
        &self,
        asset_name: &str,
        track_name: &str,
        file: &str,
    ) -> Option<(Vec<u8>, &'static str)> {
        let asset = self.assets.get(asset_name)?;
        self.metrics
            .hls_requests_total
            .fetch_add(1, Ordering::Relaxed);

        if file == "media.m3u8" {
            // The original playlist is stored as a companion data track
            // named "<track>/media.m3u8".
            let playlist_name = format!("{track_name}/media.m3u8");
            let track = asset.tracks.iter().find(|t| t.name == playlist_name)?;
            let bytes = concat_track_segments(asset, track.track_id)?;
            return Some((bytes, "application/vnd.apple.mpegurl"));
        }

        let track = asset.tracks.iter().find(|t| {
            t.name == track_name && matches!(t.kind, TrackKind::Video | TrackKind::Audio)
        })?;

        if file == "init.mp4" {
            let mut out = Vec::new();
            for &idx in &track.init_chunks {
                out.extend_from_slice(&asset.source.read_raw(idx)?);
            }
            return Some((out, "video/mp4"));
        }
        // seg_NNNNN.m4s -> nth segment of the track in pts order.
        let ordinal: usize = file
            .strip_prefix("seg_")?
            .strip_suffix(".m4s")?
            .parse()
            .ok()?;
        let mut segs: Vec<&SegmentRecord> = asset
            .segments
            .iter()
            .filter(|s| s.track_id == track.track_id)
            .collect();
        segs.sort_by_key(|s| (s.pts_start, s.segment_id));
        let seg = segs.get(ordinal)?;
        let mut out = Vec::new();
        for &idx in &seg.chunks {
            out.extend_from_slice(&asset.source.read_raw(idx)?);
        }
        Some((out, "video/iso.segment"))
    }

    pub fn render_metrics(&self) -> String {
        let m = &self.metrics;
        let counters = [
            ("cavs_sessions_opened_total", &m.sessions_opened_total),
            ("cavs_batches_total", &m.batches_total),
            ("cavs_chunks_inline_total", &m.chunks_inline_total),
            ("cavs_refs_sent_total", &m.refs_sent_total),
            ("cavs_bytes_inline_total", &m.bytes_inline_total),
            ("cavs_bundle_collapses_total", &m.bundle_collapses_total),
            ("cavs_hls_requests_total", &m.hls_requests_total),
            ("cavs_chunk_requests_total", &m.chunk_requests_total),
            ("cavs_bootstrap_sessions_total", &m.bootstrap_sessions_total),
            ("cavs_bootstraps_served_total", &m.bootstraps_served_total),
            ("cavs_bootstrap_bytes_total", &m.bootstrap_bytes_total),
            (
                "cavs_manifest_json_requests_total",
                &m.manifest_json_requests_total,
            ),
            (
                "cavs_manifest_json_bytes_total",
                &m.manifest_json_bytes_total,
            ),
            (
                "cavs_manifest_binary_requests_total",
                &m.manifest_binary_requests_total,
            ),
            (
                "cavs_manifest_binary_bytes_total",
                &m.manifest_binary_bytes_total,
            ),
        ];
        let mut out = String::new();
        for (name, counter) in counters {
            out.push_str(&format!("# TYPE {name} counter\n"));
            out.push_str(&format!("{name} {}\n", counter.load(Ordering::Relaxed)));
        }
        out
    }
}

fn to_hex_slice(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Look for `<asset>.cavs.bootstrap.zst` next to the served file and verify
/// it against the packer's `bootstrap.size` / `bootstrap.blake3` metadata.
/// A missing or tampered sidecar simply disables the bootstrap route.
fn load_bootstrap_sidecar(
    cavs_path: &std::path::Path,
    meta: &[(String, String)],
) -> Option<Bootstrap> {
    let get = |key: &str| meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let expected_size: u64 = get("bootstrap.size")?.parse().ok()?;
    let expected_blake3 = get("bootstrap.blake3")?.to_string();

    let path = PathBuf::from(format!("{}.bootstrap.zst", cavs_path.display()));
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => {
            eprintln!(
                "[server] {}: manifest declares a bootstrap but {} is missing; \
                 serving chunks only",
                cavs_path.display(),
                path.display()
            );
            return None;
        }
    };

    // Streaming hash: bootstraps can be hundreds of MiB.
    let mut hasher = cavs_hash::Hasher::new();
    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        use std::io::Read as _;
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
                size += n as u64;
            }
            Err(_) => return None,
        }
    }
    let blake3_hex = to_hex(&hasher.finalize());
    if size != expected_size || !blake3_hex.eq_ignore_ascii_case(&expected_blake3) {
        eprintln!(
            "[server] {}: bootstrap sidecar does not match its manifest \
             (size {size} vs {expected_size}); ignoring it",
            path.display()
        );
        return None;
    }
    Some(Bootstrap {
        path,
        size,
        blake3_hex,
    })
}

fn concat_track_segments(asset: &Asset, track_id: u32) -> Option<Vec<u8>> {
    let mut segs: Vec<&SegmentRecord> = asset
        .segments
        .iter()
        .filter(|s| s.track_id == track_id)
        .collect();
    segs.sort_by_key(|s| (s.pts_start, s.segment_id));
    let mut out = Vec::new();
    for seg in segs {
        for &idx in &seg.chunks {
            out.extend_from_slice(&asset.source.read_raw(idx)?);
        }
    }
    Some(out)
}
