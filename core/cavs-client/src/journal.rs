//! Resume journal (v0.5.0): a crash-safe record of an in-flight fetch.
//!
//! One small JSON file per asset under `<cache>/journal/`, written with
//! tmp+rename so it is never torn. It records *which* fetch was running
//! and where its partial artifacts live; the byte-level truth stays in
//! the artifacts themselves (the `.part` file length, the chunk cache).
//!
//! Safety rules:
//! - A journal is only honoured when server, asset and manifest hash all
//!   match the new fetch; anything else discards the journal and its
//!   partial files and starts clean.
//! - The final output is promoted only after full verification, so an
//!   interrupted run can at worst leave a `.part` and a journal behind —
//!   never a wrong file.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const JOURNAL_DIR: &str = "journal";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResumeState {
    /// The bootstrap artifact is (partially) downloaded to `bootstrap_part`.
    BootstrapDownloading,
    /// On the chunk route: progress lives in the chunk cache itself, so a
    /// rerun re-announces the have-set and only fetches what is missing.
    ChunkDownloading,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeJournal {
    pub asset: String,
    pub server: String,
    pub output: PathBuf,
    /// BLAKE3 (hex) of the manifest bytes this fetch was planned against;
    /// a republished asset invalidates the journal.
    pub manifest_blake3: String,
    pub state: ResumeState,
    /// The bootstrap `.zst.part` file being downloaded, when on that route.
    pub bootstrap_part: Option<PathBuf>,
    /// Expected BLAKE3 (hex) of the *complete* bootstrap artifact.
    pub bootstrap_blake3: Option<String>,
    /// Unix seconds of the last state change (informational).
    pub updated_at: u64,
}

impl ResumeJournal {
    pub fn path(cache_dir: &Path, asset: &str) -> PathBuf {
        cache_dir
            .join(JOURNAL_DIR)
            .join(format!("{}.resume.json", safe_name(asset)))
    }

    pub fn load(cache_dir: &Path, asset: &str) -> Option<Self> {
        let bytes = std::fs::read(Self::path(cache_dir, asset)).ok()?;
        // A corrupt journal is not an error: resuming is best-effort.
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, cache_dir: &Path) -> Result<()> {
        let path = Self::path(cache_dir, &self.asset);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing journal {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Remove the journal and any partial artifact it references.
    pub fn discard(&self, cache_dir: &Path) {
        if let Some(part) = &self.bootstrap_part {
            let _ = std::fs::remove_file(part);
        }
        let _ = std::fs::remove_file(Self::path(cache_dir, &self.asset));
    }

    /// Remove just the journal file (fetch completed; nothing partial left).
    pub fn clear(cache_dir: &Path, asset: &str) {
        let _ = std::fs::remove_file(Self::path(cache_dir, asset));
    }

    /// Every readable journal in the cache, for `cavs-client resume`.
    pub fn list(cache_dir: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(cache_dir.join(JOURNAL_DIR)) else {
            return Vec::new();
        };
        let mut out: Vec<Self> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        out.sort_by(|a, b| a.asset.cmp(&b.asset));
        out
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Journal filenames come from server-provided asset names: keep them flat.
fn safe_name(asset: &str) -> String {
    asset
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal(asset: &str) -> ResumeJournal {
        ResumeJournal {
            asset: asset.to_string(),
            server: "http://127.0.0.1:8990".into(),
            output: PathBuf::from("/tmp/out"),
            manifest_blake3: "ab".repeat(32),
            state: ResumeState::ChunkDownloading,
            bootstrap_part: None,
            bootstrap_blake3: None,
            updated_at: now_unix(),
        }
    }

    #[test]
    fn save_load_list_clear_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let j = journal("game_v2");
        j.save(dir.path()).unwrap();
        let loaded = ResumeJournal::load(dir.path(), "game_v2").unwrap();
        assert_eq!(loaded.asset, "game_v2");
        assert_eq!(loaded.state, ResumeState::ChunkDownloading);
        assert_eq!(ResumeJournal::list(dir.path()).len(), 1);

        ResumeJournal::clear(dir.path(), "game_v2");
        assert!(ResumeJournal::load(dir.path(), "game_v2").is_none());
        assert!(ResumeJournal::list(dir.path()).is_empty());
    }

    #[test]
    fn discard_removes_partial_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("x.bootstrap.zst.part");
        std::fs::write(&part, b"partial").unwrap();
        let mut j = journal("game_v3");
        j.state = ResumeState::BootstrapDownloading;
        j.bootstrap_part = Some(part.clone());
        j.save(dir.path()).unwrap();

        ResumeJournal::load(dir.path(), "game_v3")
            .unwrap()
            .discard(dir.path());
        assert!(!part.exists());
        assert!(ResumeJournal::load(dir.path(), "game_v3").is_none());
    }

    #[test]
    fn corrupt_journal_reads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = ResumeJournal::path(dir.path(), "bad");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        assert!(ResumeJournal::load(dir.path(), "bad").is_none());
    }

    #[test]
    fn hostile_asset_names_stay_flat() {
        let dir = tempfile::tempdir().unwrap();
        let j = journal("../../etc/passwd");
        j.save(dir.path()).unwrap();
        // The file must land inside the journal dir, not escape it.
        let listed = ResumeJournal::list(dir.path());
        assert_eq!(listed.len(), 1);
        assert!(ResumeJournal::path(dir.path(), "../../etc/passwd")
            .starts_with(dir.path().join(JOURNAL_DIR)));
    }
}
