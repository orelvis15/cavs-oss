//! Session-scoped access to the remote's GlobalStore: one exclusive lock and
//! one open store per push session (git-lfs sends every object of a push
//! through the same agent process), so a 250-object push pays for one store
//! open. Publishes are batched Xet-style: uploads only ingest, and the
//! session [`WriteSession::finalize`] (at terminate) commits the ledger once
//! and exports every uploaded asset — packs aggregate across objects up to
//! the store's preferred pack size instead of one pack per object, and
//! `index.json` is written once per push instead of once per object.

use anyhow::{Context, Result};
use cavs_store::{GlobalStore, StoreLayout};
use std::path::Path;

/// Exclusive advisory lock on `<tree>/.store.lock`. Held for the whole
/// push session; released when dropped (the OS also releases it if the
/// process dies). Note: advisory file locks are unreliable on some network
/// filesystems (NFS) — see the crate README.
pub struct StoreLock {
    _file: std::fs::File,
}

impl StoreLock {
    pub fn acquire(tree: &Path) -> Result<Self> {
        let path = tree.join(".store.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open lock file {}", path.display()))?;
        file.lock()
            .with_context(|| format!("cannot lock {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

/// The write half of an upload session: lock + open store (with an open
/// publish batch), created on the first upload event; finalized and dropped
/// at terminate.
pub struct WriteSession {
    _lock: StoreLock,
    pub store: GlobalStore,
    /// The export tree of the directory remote.
    pub tree: std::path::PathBuf,
    /// Oids ingested (or found missing from the export tree) this session,
    /// exported by [`Self::finalize`].
    pub pending_exports: Vec<String>,
}

pub fn open_session(tree: &Path, store_dir: &Path) -> Result<WriteSession> {
    std::fs::create_dir_all(tree)?;
    // GlobalStore has no internal locking; serialize writers across
    // processes (a concurrent `git push` from elsewhere blocks here).
    let lock = StoreLock::acquire(tree)?;
    let mut store = GlobalStore::open_with_layout(store_dir, Some(StoreLayout::Packfiles))?;
    store.begin_publish_batch();
    Ok(WriteSession {
        _lock: lock,
        store,
        tree: tree.to_path_buf(),
        pending_exports: Vec::new(),
    })
}

impl WriteSession {
    /// Commit the batched publishes (one pack close + one `index.json`
    /// write for the whole push) and export every asset uploaded this
    /// session into the static tree. Idempotent.
    pub fn finalize(&mut self) -> Result<()> {
        self.store
            .commit_publish_batch()
            .context("committing publish batch")?;
        for oid in std::mem::take(&mut self.pending_exports) {
            self.store
                .export_asset(&oid, &self.tree)
                .with_context(|| format!("exporting {oid}"))?;
        }
        Ok(())
    }
}
