//! Global content-addressable store.
//!
//! One physical copy of each unique chunk across every asset and version,
//! with reference counting and garbage collection. This is what turns the
//! per-`.cavs` egress dedup into real server-side *storage* dedup: ingest
//! v1 and v2 of a game and the bytes they share are stored once.
//!
//! On-disk layout under `root/`:
//! ```text
//!   chunks/<ab>/<hex>        loose layout: one file per chunk, as stored
//!   packs/<ab>/<id>.cavspack packfile layout: chunks appended into large
//!   packs/<ab>/<id>.cavsindex  immutable packs + per-pack sidecar index
//!   assets/<name>.json       per-asset record (tracks/segments by hash)
//!   index.json               chunk ledger: hex -> {sizes, flags, refcount,
//!                            pack location}; plus the store layout
//! ```
//! Chunks are stored in their *stored* (possibly compressed) form so the
//! server can stream them to clients with zero recompression, exactly like
//! the `.cavs` DATA section.
//!
//! The **layout** is fixed at store creation: `loose` (one object per
//! chunk — the pre-0.4.0 behavior, still fully supported) or `packfiles`
//! (chunks appended into content-addressed `.cavspack` files, read by
//! range — see [`crate::packfile`]). A store never mixes semantics: the
//! ledger records where each chunk lives, and reads follow the record.

use crate::packfile::{self, PackWriter, PREFERRED_PACK_SIZE};
use cavs_hash::{from_hex, to_hex, ChunkHash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("chunk {0} not in store")]
    MissingChunk(String),
    #[error("asset {0} not found")]
    AssetNotFound(String),
    #[error("bad chunk hash {0}")]
    BadHash(String),
    #[error("invalid asset name {0}")]
    BadAssetName(String),
    #[error("corrupt packfile: {0}")]
    PackCorrupt(String),
    #[error("store uses layout {store:?}, requested {requested:?}")]
    LayoutMismatch {
        store: StoreLayout,
        requested: StoreLayout,
    },
    #[error("{0}")]
    NotExportable(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Physical chunk layout, fixed when the store is created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreLayout {
    /// One file per chunk under `chunks/<ab>/<hex>` (pre-0.4.0 behavior).
    #[default]
    Loose,
    /// Chunks appended into immutable `.cavspack` files, read by range.
    Packfiles,
}

/// Per-chunk ledger entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub len_raw: u32,
    pub len_stored: u32,
    pub flags: u32,
    pub refcount: u64,
    /// Unix epoch seconds when refcount last hit 0 (GC grace anchor).
    #[serde(default)]
    pub zero_since: Option<u64>,
    /// Packfile id (hex) holding this chunk; absent for loose chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    /// Offset into the pack's data region, when `pack` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_offset: Option<u64>,
}

/// Where a chunk physically lives, for manifest location hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLocation {
    pub pack_hex: String,
    pub offset: u64,
    pub stored_len: u32,
}

/// Read-efficiency counters of one coalesced batch read.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoalesceStats {
    /// Chunk payloads requested from packfiles.
    pub pack_chunks_requested: u64,
    /// Physical range reads actually issued to packfiles.
    pub pack_ranges_read: u64,
    /// Bytes read from packfiles (≥ bytes served when gaps are included).
    pub pack_bytes_read: u64,
    /// Chunk payload bytes served from packfiles.
    pub pack_bytes_served: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreTrack {
    pub track_id: u32,
    pub kind: u8,
    pub codec: String,
    pub name: String,
    pub timescale: u32,
    pub init_chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSegment {
    pub segment_id: u64,
    pub track_id: u32,
    pub pts_start: u64,
    pub duration: u32,
    pub random_access: bool,
    pub chunks: Vec<String>,
}

/// Everything needed to serve an asset from the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRecord {
    pub name: String,
    pub asset_uuid: String,
    pub tracks: Vec<StoreTrack>,
    pub segments: Vec<StoreSegment>,
    pub dict: Vec<String>,
    pub chunk_table: Vec<String>,
    pub merkle_root: String,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub signer_pubkey: Option<String>,
    #[serde(default)]
    pub meta: Vec<(String, String)>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    /// hex -> chunk ledger entry. BTreeMap for stable, diff-friendly json.
    chunks: BTreeMap<String, ChunkInfo>,
    /// asset name -> distinct chunk hexes it references (refcount ledger).
    assets: BTreeMap<String, Vec<String>>,
    /// Physical layout; absent in pre-0.4.0 stores (= loose).
    #[serde(default)]
    layout: StoreLayout,
}

/// Summary for `store stat`.
#[derive(Debug, Clone)]
pub struct StoreStats {
    pub assets: usize,
    pub unique_chunks: u64,
    pub stored_bytes: u64,
    pub unique_raw_bytes: u64,
    /// Bytes that would be stored if every asset kept its own copy.
    pub logical_stored_bytes: u64,
    pub zero_ref_chunks: u64,
    pub layout: StoreLayout,
    /// Packfile layout only: pack files on disk and their total size.
    pub pack_count: u64,
    pub pack_disk_bytes: u64,
    /// Stored bytes of live (referenced) chunks inside packs; the gap to
    /// `pack_disk_bytes` is dead weight reclaimable when a pack fully dies.
    pub pack_live_bytes: u64,
}

pub struct GlobalStore {
    root: PathBuf,
    index: Index,
    open_pack: Option<PackWriter>,
    preferred_pack_size: u64,
}

impl GlobalStore {
    /// Open (or create) a store rooted at `root`, keeping its layout.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_layout(root, None)
    }

    /// Open a store; `layout` is applied only when the store is newly
    /// created. Opening an existing store with a *different* requested
    /// layout is an error (a store never changes layout in place).
    pub fn open_with_layout(root: &Path, layout: Option<StoreLayout>) -> Result<Self> {
        std::fs::create_dir_all(root.join("chunks"))?;
        std::fs::create_dir_all(root.join("assets"))?;
        let index_path = root.join("index.json");
        let index = if index_path.exists() {
            let index: Index = serde_json::from_slice(&std::fs::read(&index_path)?)?;
            if let Some(requested) = layout {
                if requested != index.layout {
                    return Err(StoreError::LayoutMismatch {
                        store: index.layout,
                        requested,
                    });
                }
            }
            index
        } else {
            let index = Index {
                layout: layout.unwrap_or_default(),
                ..Index::default()
            };
            // Persist immediately: the layout is a creation-time property
            // and must survive even if nothing is published yet.
            let tmp = index_path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&index)?)?;
            std::fs::rename(&tmp, &index_path)?;
            index
        };
        // A crash mid-ingest can leave a temp pack behind; it was never
        // referenced by the ledger, so it is safe to drop.
        let packs_dir = root.join("packs");
        if packs_dir.is_dir() {
            for entry in std::fs::read_dir(&packs_dir)?.flatten() {
                if entry.path().extension().is_some_and(|e| e == "part") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            index,
            open_pack: None,
            preferred_pack_size: PREFERRED_PACK_SIZE,
        })
    }

    pub fn layout(&self) -> StoreLayout {
        self.index.layout
    }

    /// Override the pack rollover size (tests use small packs).
    pub fn set_preferred_pack_size(&mut self, bytes: u64) {
        self.preferred_pack_size = bytes.max(1);
    }

    fn chunk_path(&self, hex: &str) -> PathBuf {
        self.root.join("chunks").join(&hex[..2]).join(hex)
    }

    fn packs_dir(&self) -> PathBuf {
        self.root.join("packs")
    }

    pub fn has_chunk(&self, hash: &ChunkHash) -> bool {
        self.index.chunks.contains_key(&to_hex(hash))
    }

    pub fn chunk_info(&self, hash: &ChunkHash) -> Option<&ChunkInfo> {
        self.index.chunks.get(&to_hex(hash))
    }

    /// Store a chunk in its stored form. No-op (returns false) if already
    /// present. New chunks enter with refcount 0 until an asset is published.
    ///
    /// In the packfile layout the chunk is appended to the currently open
    /// pack; its ledger location is resolved when the pack closes (on
    /// rollover, or at the latest inside [`Self::publish_asset`]).
    pub fn put_chunk(
        &mut self,
        hash: &ChunkHash,
        stored: &[u8],
        flags: u32,
        len_raw: u32,
    ) -> Result<bool> {
        let hex = to_hex(hash);
        if self.index.chunks.contains_key(&hex) {
            return Ok(false);
        }
        let entry = ChunkInfo {
            len_raw,
            len_stored: stored.len() as u32,
            flags,
            refcount: 0,
            zero_since: Some(0),
            pack: None,
            pack_offset: None,
        };
        match self.index.layout {
            StoreLayout::Loose => {
                let path = self.chunk_path(&hex);
                std::fs::create_dir_all(path.parent().unwrap())?;
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, stored)?;
                std::fs::rename(&tmp, &path)?;
                self.index.chunks.insert(hex, entry);
            }
            StoreLayout::Packfiles => {
                if self.open_pack.is_none() {
                    self.open_pack = Some(PackWriter::create(&self.packs_dir())?);
                }
                let writer = self.open_pack.as_mut().unwrap();
                writer.append(*hash, stored, len_raw, flags)?;
                let full = writer.data_len() >= self.preferred_pack_size;
                // Ledger entry first (location unresolved), so the flush
                // below — and any later one — fills in pack/offset.
                self.index.chunks.insert(hex, entry);
                if full {
                    self.flush_packs()?;
                }
            }
        }
        Ok(true)
    }

    /// Close the currently open pack, if any, resolving the ledger
    /// locations of every chunk it holds. Idempotent.
    pub fn flush_packs(&mut self) -> Result<()> {
        let Some(writer) = self.open_pack.take() else {
            return Ok(());
        };
        if writer.is_empty() {
            writer.abort();
            return Ok(());
        }
        let (pack_hex, entries) = writer.finish()?;
        for entry in entries {
            let hex = to_hex(&entry.hash);
            match self.index.chunks.get_mut(&hex) {
                Some(info) => {
                    info.pack = Some(pack_hex.clone());
                    info.pack_offset = Some(entry.offset);
                }
                // put_chunk always inserts the entry before flushing, so
                // this arm is defensive (e.g. a future caller flushing a
                // writer it fed directly).
                None => {
                    self.index.chunks.insert(
                        hex,
                        ChunkInfo {
                            len_raw: entry.raw_len,
                            len_stored: entry.stored_len,
                            flags: entry.flags,
                            refcount: 0,
                            zero_since: Some(0),
                            pack: Some(pack_hex.clone()),
                            pack_offset: Some(entry.offset),
                        },
                    );
                }
            }
        }
        Ok(())
    }

    /// Read a chunk in its stored form: (stored bytes, flags, len_raw).
    pub fn read_chunk_stored(&self, hash: &ChunkHash) -> Result<(Vec<u8>, u32, u32)> {
        let hex = to_hex(hash);
        let info = self
            .index
            .chunks
            .get(&hex)
            .ok_or_else(|| StoreError::MissingChunk(hex.clone()))?;
        let bytes = match (&info.pack, info.pack_offset) {
            (Some(pack), Some(offset)) => packfile::read_pack_range(
                &packfile::pack_path(&self.packs_dir(), pack),
                offset,
                info.len_stored as u64,
            )?,
            _ => std::fs::read(self.chunk_path(&hex))
                .map_err(|_| StoreError::MissingChunk(hex.clone()))?,
        };
        Ok((bytes, info.flags, info.len_raw))
    }

    /// Where a chunk physically lives, when it lives in a pack (manifest
    /// location hints).
    pub fn chunk_location(&self, hash: &ChunkHash) -> Option<ChunkLocation> {
        let info = self.index.chunks.get(&to_hex(hash))?;
        Some(ChunkLocation {
            pack_hex: info.pack.clone()?,
            offset: info.pack_offset?,
            stored_len: info.len_stored,
        })
    }

    /// Maximum dead space between two chunks that still coalesces into one
    /// physical read.
    const MAX_COALESCE_GAP: u64 = 64 * 1024;
    /// Upper bound of one coalesced read.
    const MAX_COALESCED_RANGE: u64 = 8 * 1024 * 1024;

    /// Read many chunks (stored form), coalescing pack reads: chunks from
    /// the same pack whose ranges are within [`Self::MAX_COALESCE_GAP`] of
    /// each other are fetched with a single physical read (capped at
    /// [`Self::MAX_COALESCED_RANGE`]). Results keep the input order; loose
    /// chunks read individually. Returns per-batch efficiency counters.
    #[allow(clippy::type_complexity)]
    pub fn read_chunks_stored_batch(
        &self,
        hashes: &[ChunkHash],
    ) -> Result<(Vec<(Vec<u8>, u32, u32)>, CoalesceStats)> {
        let mut out: Vec<Option<(Vec<u8>, u32, u32)>> = vec![None; hashes.len()];
        let mut stats = CoalesceStats::default();
        // pack hex -> (input position, offset, stored_len, flags, len_raw)
        let mut by_pack: HashMap<&str, Vec<(usize, u64, u32, u32, u32)>> = HashMap::new();

        for (pos, hash) in hashes.iter().enumerate() {
            let hex = to_hex(hash);
            let info = self
                .index
                .chunks
                .get(&hex)
                .ok_or_else(|| StoreError::MissingChunk(hex.clone()))?;
            match (&info.pack, info.pack_offset) {
                (Some(pack), Some(offset)) => {
                    by_pack.entry(pack.as_str()).or_default().push((
                        pos,
                        offset,
                        info.len_stored,
                        info.flags,
                        info.len_raw,
                    ));
                }
                _ => {
                    let bytes = std::fs::read(self.chunk_path(&hex))
                        .map_err(|_| StoreError::MissingChunk(hex.clone()))?;
                    out[pos] = Some((bytes, info.flags, info.len_raw));
                }
            }
        }

        for (pack, mut chunks) in by_pack {
            let pack_file = packfile::pack_path(&self.packs_dir(), pack);
            chunks.sort_by_key(|&(_, offset, ..)| offset);
            stats.pack_chunks_requested += chunks.len() as u64;

            let mut i = 0;
            while i < chunks.len() {
                // Grow the range while the next chunk is close enough and
                // the merged read stays under the cap.
                let start = chunks[i].1;
                let mut end = chunks[i].1 + chunks[i].2 as u64;
                let mut j = i + 1;
                while j < chunks.len() {
                    let (_, offset, stored_len, ..) = chunks[j];
                    let chunk_end = offset + stored_len as u64;
                    if offset.saturating_sub(end) > Self::MAX_COALESCE_GAP
                        || chunk_end.max(end) - start > Self::MAX_COALESCED_RANGE
                    {
                        break;
                    }
                    end = end.max(chunk_end);
                    j += 1;
                }
                let range = packfile::read_pack_range(&pack_file, start, end - start)?;
                stats.pack_ranges_read += 1;
                stats.pack_bytes_read += end - start;
                for &(pos, offset, stored_len, flags, len_raw) in &chunks[i..j] {
                    let lo = (offset - start) as usize;
                    let bytes = range[lo..lo + stored_len as usize].to_vec();
                    stats.pack_bytes_served += stored_len as u64;
                    out[pos] = Some((bytes, flags, len_raw));
                }
                i = j;
            }
        }

        Ok((out.into_iter().map(|c| c.unwrap()).collect(), stats))
    }

    /// Publish (or replace) an asset. Refcounts are adjusted so the chunk
    /// ledger reflects exactly the currently-published assets.
    pub fn publish_asset(&mut self, record: &AssetRecord) -> Result<()> {
        if record.name.contains(['/', '\\', '.']) || record.name.is_empty() {
            return Err(StoreError::BadAssetName(record.name.clone()));
        }
        // Close the ingest pack so every chunk has a resolved location
        // before the ledger is persisted.
        self.flush_packs()?;
        // Distinct chunks this asset references.
        let mut distinct: HashSet<String> = HashSet::new();
        for t in &record.tracks {
            distinct.extend(t.init_chunks.iter().cloned());
        }
        for s in &record.segments {
            distinct.extend(s.chunks.iter().cloned());
        }
        // Validate every referenced chunk exists.
        for hex in &distinct {
            if !self.index.chunks.contains_key(hex) {
                return Err(StoreError::MissingChunk(hex.clone()));
            }
        }
        // Replacing: drop old refs first.
        if let Some(old) = self.index.assets.remove(&record.name) {
            self.decrement(&old);
        }
        for hex in &distinct {
            if let Some(info) = self.index.chunks.get_mut(hex) {
                info.refcount += 1;
                info.zero_since = None;
            }
        }
        self.index
            .assets
            .insert(record.name.clone(), distinct.into_iter().collect());
        let json = serde_json::to_vec_pretty(record)?;
        let path = self
            .root
            .join("assets")
            .join(format!("{}.json", record.name));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &path)?;
        self.save_index()
    }

    /// Unpublish an asset: drop its references (chunks may become zero-ref,
    /// reclaimable by `gc`). Returns false if the asset was not present.
    pub fn unpublish_asset(&mut self, name: &str) -> Result<bool> {
        let Some(chunks) = self.index.assets.remove(name) else {
            return Ok(false);
        };
        self.decrement(&chunks);
        let path = self.root.join("assets").join(format!("{name}.json"));
        let _ = std::fs::remove_file(path);
        self.save_index()?;
        Ok(true)
    }

    fn decrement(&mut self, chunks: &[String]) {
        for hex in chunks {
            if let Some(info) = self.index.chunks.get_mut(hex) {
                info.refcount = info.refcount.saturating_sub(1);
                if info.refcount == 0 {
                    // Stamped 0 as a sentinel; real epoch set by caller-aware
                    // paths is unnecessary — gc uses now vs zero_since.
                    info.zero_since = Some(now_epoch());
                }
            }
        }
    }

    /// Remove chunks that have had refcount 0 for at least `grace_secs`.
    /// Returns (chunks removed, bytes reclaimed).
    ///
    /// Packfiles are immutable, so a packed chunk is only *logically*
    /// removed (its ledger entry disappears); the pack file itself is
    /// deleted — together with its sidecar index — once **no live ledger
    /// entry references it** (the roadmap's zero-live-pack policy; partial
    /// compaction is deliberately out of scope for 0.4.0).
    pub fn gc(&mut self, grace_secs: u64) -> Result<(u64, u64)> {
        let now = now_epoch();
        let doomed: Vec<String> = self
            .index
            .chunks
            .iter()
            .filter(|(_, i)| i.refcount == 0)
            .filter(|(_, i)| now.saturating_sub(i.zero_since.unwrap_or(0)) >= grace_secs)
            .map(|(h, _)| h.clone())
            .collect();
        let mut bytes = 0u64;
        let mut touched_packs: HashSet<String> = HashSet::new();
        for hex in &doomed {
            if let Some(info) = self.index.chunks.remove(hex) {
                match info.pack {
                    Some(pack) => {
                        touched_packs.insert(pack);
                    }
                    None => {
                        bytes += info.len_stored as u64;
                        let _ = std::fs::remove_file(self.chunk_path(hex));
                    }
                }
            }
        }
        // Delete packs that no remaining chunk references.
        if !touched_packs.is_empty() {
            let live: HashSet<&str> = self
                .index
                .chunks
                .values()
                .filter_map(|i| i.pack.as_deref())
                .collect();
            for pack in &touched_packs {
                if !live.contains(pack.as_str()) {
                    let path = packfile::pack_path(&self.packs_dir(), pack);
                    if let Ok(meta) = std::fs::metadata(&path) {
                        bytes += meta.len();
                    }
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(packfile::index_path(&self.packs_dir(), pack));
                }
            }
        }
        self.save_index()?;
        Ok((doomed.len() as u64, bytes))
    }

    pub fn asset_names(&self) -> Vec<String> {
        self.index.assets.keys().cloned().collect()
    }

    pub fn get_asset(&self, name: &str) -> Result<AssetRecord> {
        let path = self.root.join("assets").join(format!("{name}.json"));
        let bytes =
            std::fs::read(&path).map_err(|_| StoreError::AssetNotFound(name.to_string()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn stats(&self) -> StoreStats {
        let unique_chunks = self.index.chunks.len() as u64;
        let stored_bytes: u64 = self
            .index
            .chunks
            .values()
            .map(|i| i.len_stored as u64)
            .sum();
        let unique_raw_bytes: u64 = self.index.chunks.values().map(|i| i.len_raw as u64).sum();
        let zero_ref_chunks = self
            .index
            .chunks
            .values()
            .filter(|i| i.refcount == 0)
            .count() as u64;
        // Logical = if every asset stored its own copy of every chunk.
        let mut logical = 0u64;
        for chunks in self.index.assets.values() {
            for hex in chunks {
                if let Some(i) = self.index.chunks.get(hex) {
                    logical += i.len_stored as u64;
                }
            }
        }
        let mut pack_ids: HashSet<&str> = HashSet::new();
        let mut pack_live_bytes = 0u64;
        for info in self.index.chunks.values() {
            if let Some(pack) = info.pack.as_deref() {
                pack_ids.insert(pack);
                pack_live_bytes += info.len_stored as u64;
            }
        }
        let pack_disk_bytes: u64 = pack_ids
            .iter()
            .filter_map(|p| std::fs::metadata(packfile::pack_path(&self.packs_dir(), p)).ok())
            .map(|m| m.len())
            .sum();
        StoreStats {
            assets: self.index.assets.len(),
            unique_chunks,
            stored_bytes,
            unique_raw_bytes,
            logical_stored_bytes: logical,
            zero_ref_chunks,
            layout: self.index.layout,
            pack_count: pack_ids.len() as u64,
            pack_disk_bytes,
            pack_live_bytes,
        }
    }

    /// Verify: every ledger chunk reads back (loose file or pack range),
    /// decompresses when stored with zstd, and re-hashes to its identity;
    /// every referenced pack passes its header/footer check. Returns the
    /// number of chunks checked.
    pub fn verify(&self) -> Result<u64> {
        // Cap decompression by the ledger's own raw length, itself sane-
        // bounded so a corrupt ledger cannot request a huge allocation.
        const MAX_RAW: u64 = 256 * 1024 * 1024;
        for hex in self.index.chunks.keys() {
            let hash = from_hex(hex).ok_or_else(|| StoreError::BadHash(hex.clone()))?;
            let (stored, flags, len_raw) = self.read_chunk_stored(&hash)?;
            let raw = if flags & 1 != 0 {
                // CHUNK_FLAG_ZSTD == 1 (cavs-format), kept as a plain bit
                // here to avoid a dependency cycle.
                if len_raw as u64 > MAX_RAW {
                    return Err(StoreError::BadHash(format!("{hex}: raw length too large")));
                }
                zstd::bulk::decompress(&stored, len_raw as usize)
                    .map_err(|e| StoreError::BadHash(format!("{hex}: zstd: {e}")))?
            } else {
                stored
            };
            if raw.len() != len_raw as usize || cavs_hash::hash_chunk(&raw) != hash {
                return Err(StoreError::BadHash(hex.clone()));
            }
        }
        let packs: HashSet<&str> = self
            .index
            .chunks
            .values()
            .filter_map(|i| i.pack.as_deref())
            .collect();
        for pack in packs {
            packfile::verify_pack(&packfile::pack_path(&self.packs_dir(), pack))?;
        }
        Ok(self.index.chunks.len() as u64)
    }

    /// Export the store as a deterministic, immutable object tree ready to
    /// upload to object storage / a CDN:
    ///
    /// ```text
    /// out/
    ///   chunks/packs/<ab>/<id>.cavspack     immutable (content-addressed)
    ///   chunks/indexes/<ab>/<id>.cavsindex  immutable
    ///   assets/<name>/record.json           mutable per release
    /// ```
    ///
    /// Requires the packfile layout with every live chunk packed. Returns
    /// the relative paths written, packs first.
    pub fn export_object_store(&self, out: &Path) -> Result<Vec<String>> {
        if self.index.layout != StoreLayout::Packfiles {
            return Err(StoreError::NotExportable(
                "object-store export requires a packfile-layout store".into(),
            ));
        }
        if let Some((hex, _)) = self.index.chunks.iter().find(|(_, i)| i.pack.is_none()) {
            return Err(StoreError::NotExportable(format!(
                "chunk {hex} is not packed (ingest still open?)"
            )));
        }
        let packs: HashSet<&str> = self
            .index
            .chunks
            .values()
            .filter_map(|i| i.pack.as_deref())
            .collect();
        let mut written = Vec::new();
        let mut packs: Vec<&str> = packs.into_iter().collect();
        packs.sort_unstable();
        for pack in packs {
            for (src, rel) in [
                (
                    packfile::pack_path(&self.packs_dir(), pack),
                    format!("chunks/packs/{}/{pack}.cavspack", &pack[..2]),
                ),
                (
                    packfile::index_path(&self.packs_dir(), pack),
                    format!("chunks/indexes/{}/{pack}.cavsindex", &pack[..2]),
                ),
            ] {
                let dst = out.join(&rel);
                std::fs::create_dir_all(dst.parent().unwrap())?;
                // Packs and their indexes are immutable and content-addressed:
                // if the destination already exists with the same length it is
                // the same object, so skip the copy. This makes re-exporting
                // into the same tree effectively incremental.
                let same = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
                    (Ok(s), Ok(d)) => s.len() == d.len(),
                    _ => false,
                };
                if !same {
                    std::fs::copy(&src, &dst)?;
                }
                written.push(rel);
            }
        }
        for name in self.index.assets.keys() {
            let rel = format!("assets/{name}/record.json");
            let dst = out.join(&rel);
            std::fs::create_dir_all(dst.parent().unwrap())?;
            std::fs::copy(self.root.join("assets").join(format!("{name}.json")), &dst)?;
            written.push(rel);
        }
        Ok(written)
    }

    /// v0.6.0 static/CDN compatibility: write one `chunk-map.json` per
    /// asset into an exported object tree. It maps every chunk the asset
    /// references to its immutable pack file and byte range, so a client
    /// against a *static* HTTP host can plan a fetch (compute its missing
    /// set, then issue pack range requests) with no smart server at all.
    pub fn export_static_plans(&self, out: &Path) -> Result<Vec<String>> {
        if self.index.layout != StoreLayout::Packfiles {
            return Err(StoreError::NotExportable(
                "static plans require a packfile-layout store".into(),
            ));
        }
        let mut written = Vec::new();
        for (name, hexes) in &self.index.assets {
            let mut chunks = Vec::with_capacity(hexes.len());
            for hex in hexes {
                let Some(info) = self.index.chunks.get(hex) else {
                    continue;
                };
                let Some(pack) = info.pack.as_deref() else {
                    return Err(StoreError::NotExportable(format!(
                        "chunk {hex} is not packed (ingest still open?)"
                    )));
                };
                // `pack_offset` is into the pack's data region; a static
                // client that knows nothing about the packfile header wants
                // the absolute file offset for its HTTP Range request, so we
                // publish both.
                let pack_offset = info.pack_offset.unwrap_or(0);
                chunks.push(serde_json::json!({
                    "hash": hex,
                    "len_raw": info.len_raw,
                    "len_stored": info.len_stored,
                    "flags": info.flags,
                    "pack": format!("chunks/packs/{}/{pack}.cavspack", &pack[..2]),
                    "pack_offset": pack_offset,
                    "pack_offset_abs": packfile::PACK_HEADER_LEN + pack_offset,
                }));
            }
            let rel = format!("assets/{name}/chunk-map.json");
            let dst = out.join(&rel);
            std::fs::create_dir_all(dst.parent().unwrap())?;
            std::fs::write(
                &dst,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "asset": name,
                    "chunks": chunks,
                }))?,
            )?;
            written.push(rel);
        }
        Ok(written)
    }

    /// Build the runtime [`cavs_proto::Manifest`] for a stored asset (the
    /// reconstruction structure a client needs: ordered chunks per
    /// track/segment, with each chunk's raw length pulled from the store
    /// ledger). Mirrors the server's `AppState::manifest`, but reads from an
    /// [`AssetRecord`] + the chunk ledger so a *serverless* client can plan a
    /// fetch from a static export.
    pub fn asset_manifest(&self, name: &str) -> Result<cavs_proto::Manifest> {
        let record = self.get_asset(name)?;
        let chunk_ref = |hex: &str| {
            let len = from_hex(hex)
                .and_then(|h| self.chunk_info(&h))
                .map(|i| i.len_raw)
                .unwrap_or(0);
            cavs_proto::ChunkRef {
                hash: hex.to_string(),
                len,
            }
        };
        // Track kind labels as encoded by the `.cavs` container (see
        // `cavs_format::TrackKind`); re-stated locally because cavs-format
        // depends on this crate, so we cannot depend on it back.
        let kind_label = |kind: u8| match kind {
            0 => "video",
            1 => "audio",
            2 => "subtitle",
            _ => "data",
        };
        Ok(cavs_proto::Manifest {
            asset: record.name.clone(),
            asset_uuid: record.asset_uuid.clone(),
            tracks: record
                .tracks
                .iter()
                .map(|t| cavs_proto::ManifestTrack {
                    track_id: t.track_id,
                    kind: kind_label(t.kind).to_string(),
                    codec: t.codec.clone(),
                    name: t.name.clone(),
                    timescale: t.timescale,
                    init_chunks: t.init_chunks.iter().map(|h| chunk_ref(h)).collect(),
                })
                .collect(),
            segments: record
                .segments
                .iter()
                .map(|s| cavs_proto::ManifestSegment {
                    segment_id: s.segment_id,
                    track_id: s.track_id,
                    pts_start: s.pts_start,
                    duration: s.duration,
                    random_access: s.random_access,
                    chunks: s.chunks.iter().map(|h| chunk_ref(h)).collect(),
                })
                .collect(),
            dict: record.dict.clone(),
            chunk_table: record.chunk_table.clone(),
            merkle_root: record.merkle_root.clone(),
            signature: record.signature.clone(),
            signer_pubkey: record.signer_pubkey.clone(),
            meta: record.meta.clone(),
        })
    }

    /// Write `assets/<name>/manifest.json` for every asset into an export
    /// tree, so a serverless client can read the reconstruction structure
    /// with no running server. Returns the relative paths written.
    pub fn export_static_manifests(&self, out: &Path) -> Result<Vec<String>> {
        let mut written = Vec::new();
        for name in self.asset_names() {
            let manifest = self.asset_manifest(&name)?;
            let rel = format!("assets/{name}/manifest.json");
            let dst = out.join(&rel);
            std::fs::create_dir_all(dst.parent().unwrap())?;
            std::fs::write(&dst, serde_json::to_vec_pretty(&manifest)?)?;
            written.push(rel);
        }
        Ok(written)
    }

    fn save_index(&self) -> Result<()> {
        let path = self.root.join("index.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.index)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cavs_hash::hash_chunk;

    fn rec(name: &str, chunks: &[&ChunkHash]) -> AssetRecord {
        AssetRecord {
            name: name.into(),
            asset_uuid: "0".repeat(32),
            tracks: vec![],
            segments: vec![StoreSegment {
                segment_id: 0,
                track_id: 0,
                pts_start: 0,
                duration: 0,
                random_access: true,
                chunks: chunks.iter().map(|h| to_hex(h)).collect(),
            }],
            dict: vec![],
            chunk_table: chunks.iter().map(|h| to_hex(h)).collect(),
            merkle_root: String::new(),
            signature: None,
            signer_pubkey: None,
            meta: vec![],
        }
    }

    #[test]
    fn dedup_gc_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let a = vec![1u8; 1000];
        let b = vec![2u8; 1000];
        let c = vec![3u8; 1000];
        let (ha, hb, hc) = (hash_chunk(&a), hash_chunk(&b), hash_chunk(&c));

        {
            let mut store = GlobalStore::open(dir.path()).unwrap();
            // v1 = {a, b}
            assert!(store.put_chunk(&ha, &a, 0, a.len() as u32).unwrap());
            assert!(store.put_chunk(&hb, &b, 0, b.len() as u32).unwrap());
            store.publish_asset(&rec("game_v1", &[&ha, &hb])).unwrap();
            // v2 = {a, c}  — 'a' shared, stored once
            assert!(
                !store.put_chunk(&ha, &a, 0, a.len() as u32).unwrap(),
                "dup stored twice"
            );
            assert!(store.put_chunk(&hc, &c, 0, c.len() as u32).unwrap());
            store.publish_asset(&rec("game_v2", &[&ha, &hc])).unwrap();

            let s = store.stats();
            assert_eq!(s.assets, 2);
            assert_eq!(s.unique_chunks, 3); // a, b, c — not 4
            assert_eq!(store.chunk_info(&ha).unwrap().refcount, 2);
            // logical (both keep own copies) = 4 chunks; unique = 3
            assert_eq!(s.logical_stored_bytes, 4000);
            assert_eq!(s.stored_bytes, 3000);
        }

        // Reopen: index persisted.
        let mut store = GlobalStore::open(dir.path()).unwrap();
        assert_eq!(store.stats().unique_chunks, 3);
        assert!(store.get_asset("game_v1").is_ok());

        // Unpublish v1: 'b' drops to zero-ref, 'a' still referenced by v2.
        assert!(store.unpublish_asset("game_v1").unwrap());
        assert_eq!(store.chunk_info(&ha).unwrap().refcount, 1);
        assert_eq!(store.chunk_info(&hb).unwrap().refcount, 0);
        // GC with grace 0 reclaims 'b' only.
        let (removed, bytes) = store.gc(0).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(bytes, 1000);
        assert_eq!(store.stats().unique_chunks, 2);
        assert!(store.read_chunk_stored(&ha).is_ok());
        assert!(store.read_chunk_stored(&hb).is_err());
    }

    #[test]
    fn republish_replaces_refs() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = GlobalStore::open(dir.path()).unwrap();
        let a = vec![9u8; 500];
        let ha = hash_chunk(&a);
        store.put_chunk(&ha, &a, 0, 500).unwrap();
        store.publish_asset(&rec("x", &[&ha])).unwrap();
        store.publish_asset(&rec("x", &[&ha])).unwrap(); // republish
                                                         // refcount stays 1, not 2.
        assert_eq!(store.chunk_info(&ha).unwrap().refcount, 1);
    }

    #[test]
    fn missing_chunk_rejected_on_publish() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = GlobalStore::open(dir.path()).unwrap();
        let ghost = hash_chunk(b"never stored");
        assert!(matches!(
            store.publish_asset(&rec("x", &[&ghost])),
            Err(StoreError::MissingChunk(_))
        ));
    }

    fn packfile_store(dir: &Path) -> GlobalStore {
        let mut store = GlobalStore::open_with_layout(dir, Some(StoreLayout::Packfiles)).unwrap();
        store.set_preferred_pack_size(4 * 1000); // tiny packs: exercise rollover
        store
    }

    #[test]
    fn packfile_layout_roundtrip_rollover_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let chunks: Vec<Vec<u8>> = (0..10u8).map(|i| vec![i; 1000]).collect();
        let hashes: Vec<ChunkHash> = chunks.iter().map(|c| hash_chunk(c)).collect();
        {
            let mut store = packfile_store(dir.path());
            for (c, h) in chunks.iter().zip(&hashes) {
                assert!(store.put_chunk(h, c, 0, c.len() as u32).unwrap());
            }
            let refs: Vec<&ChunkHash> = hashes.iter().collect();
            store.publish_asset(&rec("game", &refs)).unwrap();

            // 10 KB of chunks at a 4 KB preferred size -> several packs.
            let stats = store.stats();
            assert_eq!(stats.layout, StoreLayout::Packfiles);
            assert!(stats.pack_count >= 2, "expected rollover: {stats:?}");
            assert_eq!(stats.pack_live_bytes, 10_000);
            // No loose chunk files were written.
            assert!(!dir
                .path()
                .join("chunks")
                .join(&to_hex(&hashes[0])[..2])
                .exists());
            store.verify().unwrap();
        }
        // Reopen: locations persisted; every chunk reads back identically.
        let store = GlobalStore::open(dir.path()).unwrap();
        assert_eq!(store.layout(), StoreLayout::Packfiles);
        for (c, h) in chunks.iter().zip(&hashes) {
            let (stored, _, _) = store.read_chunk_stored(h).unwrap();
            assert_eq!(&stored, c);
            assert!(store.chunk_location(h).is_some());
        }
    }

    #[test]
    fn coalesced_batch_read_matches_individual_reads() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = packfile_store(dir.path());
        store.set_preferred_pack_size(1 << 30); // one pack: adjacent chunks
        let chunks: Vec<Vec<u8>> = (0..50u8).map(|i| vec![i; 500]).collect();
        let hashes: Vec<ChunkHash> = chunks.iter().map(|c| hash_chunk(c)).collect();
        for (c, h) in chunks.iter().zip(&hashes) {
            store.put_chunk(h, c, 0, c.len() as u32).unwrap();
        }
        store.flush_packs().unwrap();

        // Request a scattered subset, out of order.
        let subset: Vec<ChunkHash> = [40usize, 2, 3, 4, 30, 31, 0]
            .iter()
            .map(|&i| hashes[i])
            .collect();
        let (batch, stats) = store.read_chunks_stored_batch(&subset).unwrap();
        for (got, &idx) in batch.iter().zip(&[40usize, 2, 3, 4, 30, 31, 0]) {
            assert_eq!(got.0, chunks[idx], "chunk {idx} mismatch");
        }
        // Adjacent chunks coalesce: fewer physical reads than chunks.
        assert_eq!(stats.pack_chunks_requested, 7);
        assert!(
            stats.pack_ranges_read < 7,
            "expected coalescing, got {stats:?}"
        );
        assert_eq!(stats.pack_bytes_served, 7 * 500);
        assert!(stats.pack_bytes_read >= stats.pack_bytes_served);
    }

    #[test]
    fn gc_deletes_only_fully_dead_packs() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = packfile_store(dir.path());
        // Pack 1: a+b (v1). Pack 2: c (v2, after explicit flush).
        let (a, b, c) = (vec![1u8; 1500], vec![2u8; 1500], vec![3u8; 1500]);
        let (ha, hb, hc) = (hash_chunk(&a), hash_chunk(&b), hash_chunk(&c));
        store.put_chunk(&ha, &a, 0, 1500).unwrap();
        store.put_chunk(&hb, &b, 0, 1500).unwrap();
        store.flush_packs().unwrap();
        store.put_chunk(&hc, &c, 0, 1500).unwrap();
        store.publish_asset(&rec("v1", &[&ha, &hb])).unwrap();
        store.publish_asset(&rec("v2", &[&hb, &hc])).unwrap();
        assert_eq!(store.stats().pack_count, 2);

        // Unpublish v2: 'c' dies; its pack holds only 'c' -> pack deleted.
        store.unpublish_asset("v2").unwrap();
        let (removed, bytes) = store.gc(0).unwrap();
        assert_eq!(removed, 1);
        assert!(bytes > 0, "dead pack must be reclaimed");
        assert_eq!(store.stats().pack_count, 1);
        assert!(store.read_chunk_stored(&hc).is_err());

        // Unpublish v1: 'a' and 'b' die, but they share the surviving pack
        // with nothing else -> that pack dies too.
        store.unpublish_asset("v1").unwrap();
        store.gc(0).unwrap();
        assert_eq!(store.stats().pack_count, 0);
        store.verify().unwrap();
    }

    #[test]
    fn layout_mismatch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        drop(GlobalStore::open_with_layout(dir.path(), Some(StoreLayout::Packfiles)).unwrap());
        assert!(matches!(
            GlobalStore::open_with_layout(dir.path(), Some(StoreLayout::Loose)),
            Err(StoreError::LayoutMismatch { .. })
        ));
        // Re-opening without a requested layout keeps the stored one.
        assert_eq!(
            GlobalStore::open(dir.path()).unwrap().layout(),
            StoreLayout::Packfiles
        );
    }

    #[test]
    fn export_object_store_layout_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = packfile_store(dir.path());
        let data = vec![7u8; 2000];
        let h = hash_chunk(&data);
        store.put_chunk(&h, &data, 0, 2000).unwrap();
        store.publish_asset(&rec("game", &[&h])).unwrap();

        let out = dir.path().join("dist");
        let written = store.export_object_store(&out).unwrap();
        assert!(written.iter().any(|p| p.starts_with("chunks/packs/")));
        assert!(written.iter().any(|p| p.starts_with("chunks/indexes/")));
        assert!(written.contains(&"assets/game/record.json".to_string()));
        for rel in &written {
            assert!(out.join(rel).is_file(), "{rel} missing");
        }
        // Deterministic: exporting again yields the same paths.
        let out2 = dir.path().join("dist2");
        assert_eq!(written, store.export_object_store(&out2).unwrap());
        // Loose stores are not exportable.
        let loose_dir = tempfile::tempdir().unwrap();
        let loose = GlobalStore::open(loose_dir.path()).unwrap();
        assert!(matches!(
            loose.export_object_store(&out),
            Err(StoreError::NotExportable(_))
        ));
    }

    #[test]
    fn corrupted_pack_chunk_fails_verify() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = packfile_store(dir.path());
        let data = vec![9u8; 3000];
        let h = hash_chunk(&data);
        store.put_chunk(&h, &data, 0, 3000).unwrap();
        store.publish_asset(&rec("game", &[&h])).unwrap();
        store.verify().unwrap();

        // Flip one byte inside the pack's data region.
        let pack_hex = store.chunk_location(&h).unwrap().pack_hex;
        let pack = crate::packfile::pack_path(&dir.path().join("packs"), &pack_hex);
        let mut bytes = std::fs::read(&pack).unwrap();
        bytes[crate::packfile::PACK_HEADER_LEN as usize + 100] ^= 0xff;
        std::fs::write(&pack, &bytes).unwrap();
        assert!(store.verify().is_err(), "corruption must fail verify");
    }
}
