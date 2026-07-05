//! Global content-addressable store.
//!
//! One physical copy of each unique chunk across every asset and version,
//! with reference counting and garbage collection. This is what turns the
//! per-`.cavs` egress dedup into real server-side *storage* dedup: ingest
//! v1 and v2 of a game and the bytes they share are stored once.
//!
//! On-disk layout under `root/`:
//! ```text
//!   chunks/<ab>/<hex>        chunk payload, exactly as stored (maybe zstd)
//!   assets/<name>.json       per-asset record (tracks/segments by hash)
//!   index.json               chunk ledger: hex -> {sizes, flags, refcount}
//! ```
//! Chunks are stored in their *stored* (possibly compressed) form so the
//! server can stream them to clients with zero recompression, exactly like
//! the `.cavs` DATA section.

use cavs_hash::{from_hex, to_hex, ChunkHash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
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
}

pub type Result<T> = std::result::Result<T, StoreError>;

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
}

pub struct GlobalStore {
    root: PathBuf,
    index: Index,
}

impl GlobalStore {
    /// Open (or create) a store rooted at `root`.
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root.join("chunks"))?;
        std::fs::create_dir_all(root.join("assets"))?;
        let index_path = root.join("index.json");
        let index = if index_path.exists() {
            serde_json::from_slice(&std::fs::read(&index_path)?)?
        } else {
            Index::default()
        };
        Ok(Self {
            root: root.to_path_buf(),
            index,
        })
    }

    fn chunk_path(&self, hex: &str) -> PathBuf {
        self.root.join("chunks").join(&hex[..2]).join(hex)
    }

    pub fn has_chunk(&self, hash: &ChunkHash) -> bool {
        self.index.chunks.contains_key(&to_hex(hash))
    }

    pub fn chunk_info(&self, hash: &ChunkHash) -> Option<&ChunkInfo> {
        self.index.chunks.get(&to_hex(hash))
    }

    /// Store a chunk in its stored form. No-op (returns false) if already
    /// present. New chunks enter with refcount 0 until an asset is published.
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
        let path = self.chunk_path(&hex);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, stored)?;
        std::fs::rename(&tmp, &path)?;
        self.index.chunks.insert(
            hex,
            ChunkInfo {
                len_raw,
                len_stored: stored.len() as u32,
                flags,
                refcount: 0,
                zero_since: Some(0),
            },
        );
        Ok(true)
    }

    /// Read a chunk in its stored form: (stored bytes, flags, len_raw).
    pub fn read_chunk_stored(&self, hash: &ChunkHash) -> Result<(Vec<u8>, u32, u32)> {
        let hex = to_hex(hash);
        let info = self
            .index
            .chunks
            .get(&hex)
            .ok_or_else(|| StoreError::MissingChunk(hex.clone()))?;
        let bytes = std::fs::read(self.chunk_path(&hex))
            .map_err(|_| StoreError::MissingChunk(hex.clone()))?;
        Ok((bytes, info.flags, info.len_raw))
    }

    /// Publish (or replace) an asset. Refcounts are adjusted so the chunk
    /// ledger reflects exactly the currently-published assets.
    pub fn publish_asset(&mut self, record: &AssetRecord) -> Result<()> {
        if record.name.contains(['/', '\\', '.']) || record.name.is_empty() {
            return Err(StoreError::BadAssetName(record.name.clone()));
        }
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
        for hex in &doomed {
            if let Some(info) = self.index.chunks.remove(hex) {
                bytes += info.len_stored as u64;
                let _ = std::fs::remove_file(self.chunk_path(hex));
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
        StoreStats {
            assets: self.index.assets.len(),
            unique_chunks,
            stored_bytes,
            unique_raw_bytes,
            logical_stored_bytes: logical,
            zero_ref_chunks,
        }
    }

    /// Verify: every referenced chunk exists on disk and re-hashes to its key.
    /// Returns the number of chunks checked.
    pub fn verify(&self) -> Result<u64> {
        for (hex, info) in &self.index.chunks {
            let hash = from_hex(hex).ok_or_else(|| StoreError::BadHash(hex.clone()))?;
            let stored = std::fs::read(self.chunk_path(hex))
                .map_err(|_| StoreError::MissingChunk(hex.clone()))?;
            let raw = if info.flags & 1 != 0 {
                // CHUNK_FLAG_ZSTD == 1; decode without a hard dep on cavs-format.
                return Err(StoreError::BadHash(format!(
                    "{hex}: zstd chunk verification must go through the reader"
                )));
            } else {
                stored
            };
            if cavs_hash::hash_chunk(&raw) != hash {
                return Err(StoreError::BadHash(hex.clone()));
            }
        }
        Ok(self.index.chunks.len() as u64)
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
}
