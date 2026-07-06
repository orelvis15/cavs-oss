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

    /// Every chunk entry on disk: (hex, path, len, mtime). Only the
    /// two-hex-char shard directories are chunk storage; `journal/` and
    /// `quarantine/` live alongside and are skipped.
    fn entries(&self) -> Result<Vec<(String, PathBuf, u64, std::time::SystemTime)>> {
        let mut out = Vec::new();
        for shard in std::fs::read_dir(&self.root)? {
            let shard = shard?.path();
            let is_shard = shard.is_dir()
                && shard
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_hexdigit()));
            if !is_shard {
                continue;
            }
            for entry in std::fs::read_dir(&shard)? {
                let entry = entry?;
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
                    let meta = entry.metadata()?;
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    out.push((name.to_string(), path, meta.len(), mtime));
                } else if name.ends_with(".tmp") {
                    // Torn write leftovers: always safe to remove.
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Ok(out)
    }

    /// Re-hash every entry (v0.5.0 `cache verify`). Corrupt entries are
    /// moved to `<root>/quarantine/` by default — recoverable evidence —
    /// or deleted with `delete_corrupt`. Stray `.tmp` files are removed.
    pub fn verify(&self, delete_corrupt: bool) -> Result<CacheVerifyReport> {
        let mut report = CacheVerifyReport::default();
        for (hex, path, len, _) in self.entries()? {
            report.total += 1;
            report.total_bytes += len;
            let ok = std::fs::read(&path)
                .map(|payload| to_hex(&hash_chunk(&payload)) == hex)
                .unwrap_or(false);
            if ok {
                report.ok += 1;
                continue;
            }
            report.corrupt += 1;
            if delete_corrupt {
                let _ = std::fs::remove_file(&path);
            } else {
                let qdir = self.root.join("quarantine");
                std::fs::create_dir_all(&qdir)?;
                let _ = std::fs::rename(&path, qdir.join(&hex));
            }
        }
        Ok(report)
    }

    /// Evict least-recently-modified entries until the cache fits in
    /// `max_bytes` (v0.5.0 `cache gc`).
    pub fn gc(&self, max_bytes: u64) -> Result<CacheGcReport> {
        let mut entries = self.entries()?;
        let total: u64 = entries.iter().map(|(_, _, len, _)| len).sum();
        let mut report = CacheGcReport {
            total_entries: entries.len() as u64,
            total_bytes: total,
            ..Default::default()
        };
        if total <= max_bytes {
            return Ok(report);
        }
        entries.sort_by_key(|(_, _, _, mtime)| *mtime);
        let mut current = total;
        for (_, path, len, _) in entries {
            if current <= max_bytes {
                break;
            }
            std::fs::remove_file(&path)?;
            current -= len;
            report.evicted += 1;
            report.evicted_bytes += len;
        }
        Ok(report)
    }
}

#[derive(Debug, Default)]
pub struct CacheVerifyReport {
    pub total: u64,
    pub total_bytes: u64,
    pub ok: u64,
    pub corrupt: u64,
}

#[derive(Debug, Default)]
pub struct CacheGcReport {
    pub total_entries: u64,
    pub total_bytes: u64,
    pub evicted: u64,
    pub evicted_bytes: u64,
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

    #[test]
    fn verify_quarantines_corrupt_and_cleans_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ChunkCache::open(dir.path()).unwrap();
        let good = b"good chunk".to_vec();
        let bad = b"bad chunk".to_vec();
        let good_hash = hash_chunk(&good);
        let bad_hash = hash_chunk(&bad);
        cache.put(&good_hash, &good).unwrap();
        cache.put(&bad_hash, &bad).unwrap();

        // Corrupt one entry and drop a stray .tmp next to the other.
        let bad_hex = to_hex(&bad_hash);
        let bad_path = dir.path().join(&bad_hex[..2]).join(&bad_hex);
        std::fs::write(&bad_path, b"flipped").unwrap();
        std::fs::write(bad_path.with_extension("tmp"), b"torn").unwrap();

        let report = cache.verify(false).unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.ok, 1);
        assert_eq!(report.corrupt, 1);
        assert!(!bad_path.exists());
        assert!(dir.path().join("quarantine").join(&bad_hex).exists());
        assert!(!bad_path.with_extension("tmp").exists());
        // The good entry is untouched.
        assert_eq!(cache.get(&good_hash).unwrap(), Some(good));
    }

    #[test]
    fn gc_evicts_oldest_until_under_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ChunkCache::open(dir.path()).unwrap();
        let mut hashes = Vec::new();
        for i in 0u8..4 {
            let payload = vec![i; 1000];
            let hash = hash_chunk(&payload);
            cache.put(&hash, &payload).unwrap();
            let hex = to_hex(&hash);
            // Deterministic ages: entry 0 oldest.
            let path = dir.path().join(&hex[..2]).join(&hex);
            let age =
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i as u64 * 1000);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(age)
                .unwrap();
            hashes.push(hash);
        }

        let report = cache.gc(2500).unwrap();
        assert_eq!(report.total_bytes, 4000);
        assert_eq!(report.evicted, 2);
        assert!(cache.get(&hashes[0]).unwrap().is_none());
        assert!(cache.get(&hashes[1]).unwrap().is_none());
        assert!(cache.get(&hashes[2]).unwrap().is_some());
        assert!(cache.get(&hashes[3]).unwrap().is_some());
    }
}
