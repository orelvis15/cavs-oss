//! Session-scoped access to the remote's GlobalStore: one exclusive lock and
//! one open store per push session (git-lfs sends every object of a push
//! through the same agent process), so a 250-object push pays for one store
//! open — and each upload exports only its own asset.

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

/// The write half of an upload session: lock + open store, created on the
/// first upload event and dropped at terminate.
pub struct WriteSession {
    _lock: StoreLock,
    pub store: GlobalStore,
}

pub fn open_session(tree: &Path, store_dir: &Path) -> Result<WriteSession> {
    std::fs::create_dir_all(tree)?;
    // GlobalStore has no internal locking; serialize writers across
    // processes (a concurrent `git push` from elsewhere blocks here).
    let lock = StoreLock::acquire(tree)?;
    let store = GlobalStore::open_with_layout(store_dir, Some(StoreLayout::Packfiles))?;
    Ok(WriteSession { _lock: lock, store })
}
