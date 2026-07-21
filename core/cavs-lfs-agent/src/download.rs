//! Download one LFS object: `cavs_fetch::fetch_static` against the remote's
//! static export, reconstructing into a per-object temp dir that git-lfs
//! then moves into `.git/lfs/objects`.
//!
//! The asset name at the remote IS the LFS oid, and so is the single raw
//! track's name — so the reconstructed file lands at `<tmp>/<oid>` and the
//! manifest's `sha256:<oid> = <oid>` meta entry makes `cavs-fetch` verify
//! the LFS oid end-to-end before we ever report `complete`.

use crate::protocol::{Progress, ProtoOut};
use anyhow::{anyhow, Context, Result};
use cavs_fetch::{fetch_static, FetchError, FetchOptions, StaticSource};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Fetch `oid` from the remote into a fresh temp dir under `tmp_root`.
/// Returns the reconstructed file path and the tempdir guard keeping it
/// alive (drop it only after git-lfs has consumed the file).
pub fn handle(
    fetch_base: &str,
    oid: &str,
    cache_dir: &Path,
    tmp_root: &Path,
    connections: usize,
    pubkey: Option<&str>,
    out: &ProtoOut,
) -> Result<(PathBuf, tempfile::TempDir)> {
    std::fs::create_dir_all(tmp_root)?;
    let tmpdir = tempfile::tempdir_in(tmp_root).context("creating download tempdir")?;

    let source = StaticSource::new(fetch_base);
    // Progress: cumulative wire bytes from fetch worker threads, throttled
    // so a many-chunk object does not flood git-lfs with events.
    let reported = AtomicU64::new(0);
    let progress = move |done: u64, total: u64| {
        let last = reported.load(Ordering::Relaxed);
        if done < last {
            return; // stale callback from a lagging worker thread
        }
        let step = (total / 100).max(256 * 1024);
        if done == total || done - last >= step {
            reported.store(done, Ordering::Relaxed);
            out.send(&Progress::new(oid, done, done - last));
        }
    };
    let opts = FetchOptions {
        connections,
        pubkey: pubkey.map(str::to_string),
        progress: Some(&progress),
        cancel: None,
    };

    let stats =
        fetch_static(&source, oid, tmpdir.path(), cache_dir, &opts).map_err(|e| match e {
            FetchError::Cancelled => anyhow!("fetch cancelled"),
            FetchError::Other(e) => e,
        })?;
    eprintln!(
        "[lfs-agent] download {}: {} chunks fetched, {} reused, {} wire bytes",
        &oid[..12.min(oid.len())],
        stats.fetched,
        stats.reused,
        stats.wire_bytes
    );

    let file = tmpdir.path().join(oid);
    if !file.is_file() {
        // fetch_static succeeded but the expected raw track is absent —
        // the asset at the remote was not packed by this agent.
        anyhow::bail!("asset {oid} reconstructed no file named after the oid");
    }
    Ok((file, tmpdir))
}

/// Whether the remote holds this object (used to distinguish 404 from other
/// failures on directory remotes; HTTP remotes find out via the fetch).
pub fn exists_at_dir_remote(tree: &Path, oid: &str) -> bool {
    tree.join("assets")
        .join(oid)
        .join("manifest.json")
        .is_file()
}
