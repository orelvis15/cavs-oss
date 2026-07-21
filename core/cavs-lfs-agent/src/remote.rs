//! Resolve where LFS objects live for this session.
//!
//! Precedence: `--remote` (usually injected via
//! `lfs.customtransfer.cavs.args`) → `$CAVS_LFS_REMOTE` → the remote
//! announced by git-lfs in the `init` event (a git remote name like
//! `origin`, or a URL/path).
//!
//! A directory remote is read/write; an `http(s)://` remote is read-only
//! (downloads straight off a CDN/static host). When the resolved directory
//! is a *bare git repository* the CAVS tree is placed under `<repo>/cavs/`
//! so pushing LFS objects does not pollute the bare repo root — and a fresh
//! `git clone <path>` needs zero per-remote configuration.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum Remote {
    /// Filesystem remote: `tree` is the static-export tree read by
    /// downloads; `store` is the GlobalStore that uploads ingest into.
    Dir { tree: PathBuf, store: PathBuf },
    /// Read-only static host (CDN, object storage website, `cavs serve`).
    Http(String),
}

impl Remote {
    /// The base string handed to `cavs_fetch::StaticSource::new`.
    pub fn fetch_base(&self) -> String {
        match self {
            Remote::Dir { tree, .. } => tree.display().to_string(),
            Remote::Http(base) => base.clone(),
        }
    }
}

/// Resolve the remote for this session.
pub fn resolve(cli_remote: Option<&str>, init_remote: &str) -> Result<Remote> {
    let raw = if let Some(r) = cli_remote {
        r.to_string()
    } else if let Ok(r) = std::env::var("CAVS_LFS_REMOTE") {
        r
    } else if !init_remote.is_empty() {
        // `init.remote` is either a configured git remote name or already a
        // URL/path. Try the name lookup first; fall back to literal use.
        git_remote_url(init_remote).unwrap_or_else(|| init_remote.to_string())
    } else {
        bail!("no remote: pass --remote, set CAVS_LFS_REMOTE, or configure a git remote");
    };

    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Ok(Remote::Http(raw.trim_end_matches('/').to_string()));
    }

    let path = if let Some(stripped) = raw.strip_prefix("file://") {
        PathBuf::from(stripped)
    } else {
        PathBuf::from(&raw)
    };
    // git-lfs runs the agent inside the repository, but don't bet on the
    // exact cwd: resolve relative remotes against the repo toplevel when
    // git can tell us where that is.
    let path = if path.is_relative() {
        match git_toplevel() {
            Some(top) => top.join(&path),
            None => path,
        }
    } else {
        path
    };

    let tree = if is_bare_repo(&path) {
        path.join("cavs")
    } else {
        path
    };
    Ok(Remote::Dir {
        store: tree.join(".store"),
        tree,
    })
}

/// `git remote get-url <name>` in the current repository, if it succeeds.
fn git_remote_url(name: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", name])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

fn git_toplevel() -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let top = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// A directory that looks like a bare git repository (`HEAD` + `objects/`,
/// or a `.git` suffix on an existing dir).
fn is_bare_repo(path: &Path) -> bool {
    if !path.is_dir() {
        // A remote that does not exist yet is created by the first push and
        // is definitely not a bare repo; but honor an explicit `.git` name.
        return path.extension().is_some_and(|e| e == "git");
    }
    if path.extension().is_some_and(|e| e == "git") {
        return true;
    }
    path.join("HEAD").is_file() && path.join("objects").is_dir()
}

/// Pick the chunk cache directory: `--cache-dir` → `$CAVS_LFS_CACHE` →
/// `<git-dir>/lfs/cavs/cache` → `~/.cache/cavs-lfs-agent`.
pub fn cache_dir(cli: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = cli {
        return Ok(dir.to_path_buf());
    }
    if let Ok(dir) = std::env::var("CAVS_LFS_CACHE") {
        return Ok(PathBuf::from(dir));
    }
    let git_dir = std::process::Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()));
    if let Some(git_dir) = git_dir {
        return Ok(git_dir.join("lfs").join("cavs").join("cache"));
    }
    let home = std::env::var("HOME").context("neither a git repo nor $HOME available")?;
    Ok(PathBuf::from(home).join(".cache").join("cavs-lfs-agent"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_remote_is_readonly_base() {
        let r = resolve(Some("https://cdn.example.com/lfs/"), "").unwrap();
        match r {
            Remote::Http(base) => assert_eq!(base, "https://cdn.example.com/lfs"),
            other => panic!("expected http remote, got {other:?}"),
        }
    }

    #[test]
    fn dir_remote_gets_store_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolve(Some(tmp.path().to_str().unwrap()), "").unwrap();
        match r {
            Remote::Dir { tree, store } => {
                assert_eq!(tree, tmp.path());
                assert_eq!(store, tmp.path().join(".store"));
            }
            other => panic!("expected dir remote, got {other:?}"),
        }
    }

    #[test]
    fn bare_repo_remote_appends_cavs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir(tmp.path().join("objects")).unwrap();
        let r = resolve(Some(tmp.path().to_str().unwrap()), "").unwrap();
        match r {
            Remote::Dir { tree, store } => {
                assert_eq!(tree, tmp.path().join("cavs"));
                assert_eq!(store, tmp.path().join("cavs").join(".store"));
            }
            other => panic!("expected dir remote, got {other:?}"),
        }
    }

    #[test]
    fn file_url_is_stripped() {
        let tmp = tempfile::tempdir().unwrap();
        let url = format!("file://{}", tmp.path().display());
        let r = resolve(Some(&url), "").unwrap();
        match r {
            Remote::Dir { tree, .. } => assert_eq!(tree, tmp.path()),
            other => panic!("expected dir remote, got {other:?}"),
        }
    }
}
