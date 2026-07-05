//! Persistent content-addressable chunk cache.
//!
//! Layout: `<root>/<first-2-hex-chars>/<full-hex>` — one file per chunk,
//! raw (uncompressed) payload. Reads are verified against the hash, so a
//! corrupted cache entry is treated as absent (and removed) rather than
//! poisoning reconstruction.

use anyhow::{Context, Result};
use cavs_hash::{hash_chunk, to_hex, ChunkHash};
use std::path::{Path, PathBuf};

pub struct ChunkCache {
    root: PathBuf,
}

impl ChunkCache {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create cache dir {}", root.display()))?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    fn path_for_hex(&self, hex: &str) -> PathBuf {
        self.root.join(&hex[..2]).join(hex)
    }

    pub fn contains(&self, hex: &str) -> bool {
        hex.len() == 64 && self.path_for_hex(hex).is_file()
    }

    pub fn put(&self, hash: &ChunkHash, payload: &[u8]) -> Result<()> {
        let hex = to_hex(hash);
        let path = self.path_for_hex(&hex);
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        // Write-then-rename so a crashed write never leaves a torn entry.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, payload)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Returns the verified payload, or None if absent/corrupted.
    pub fn get(&self, hash: &ChunkHash) -> Result<Option<Vec<u8>>> {
        let hex = to_hex(hash);
        let path = self.path_for_hex(&hex);
        let Ok(payload) = std::fs::read(&path) else {
            return Ok(None);
        };
        if hash_chunk(&payload) != *hash {
            // Self-heal: drop the corrupted entry.
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip_and_corruption_heals() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ChunkCache::open(dir.path()).unwrap();
        let payload = b"chunk payload".to_vec();
        let hash = hash_chunk(&payload);
        let hex = to_hex(&hash);

        assert!(!cache.contains(&hex));
        cache.put(&hash, &payload).unwrap();
        assert!(cache.contains(&hex));
        assert_eq!(cache.get(&hash).unwrap(), Some(payload));

        // Corrupt the entry on disk: get() must return None and remove it.
        let path = dir.path().join(&hex[..2]).join(&hex);
        std::fs::write(&path, b"garbage").unwrap();
        assert_eq!(cache.get(&hash).unwrap(), None);
        assert!(!cache.contains(&hex));
    }
}
