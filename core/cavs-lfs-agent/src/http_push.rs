//! Write side of an `http(s)://` remote: push the content-addressed static
//! export to a CAVS Node over the authenticated control-plane connection.
//!
//! The agent still ingests + exports into a local staging tree exactly like a
//! directory remote (so all dedup/packing logic is unchanged); this module
//! then mirrors that tree to the Hub — `HEAD` to skip bytes the remote already
//! has (wire dedup across pushes), `PUT` the rest under the same relative
//! paths `cavs-fetch` reads on download, and finally `POST .../finalize` so the
//! Hub registers the pushed LFS objects, bumps the repo generation and
//! reconciles usage (the push then shows up in the dashboard).
//!
//! ## Plaintext HTTP is gated
//!
//! Uploading sends a bearer token and repository bytes. Over `https://` that is
//! always allowed; over plaintext `http://` it is refused unless the dev escape
//! hatch `CAVS_LFS_ALLOW_INSECURE_HTTP` is set to a truthy value (`1`/`true`/
//! `yes`/`on`). This keeps the local dev stack (`http://localhost:8080`) usable
//! without ever shipping credentials in cleartext by default.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// The env var that permits pushing over plaintext `http://` (dev only).
pub const ALLOW_INSECURE_ENV: &str = "CAVS_LFS_ALLOW_INSECURE_HTTP";

/// A resolved, writable HTTP remote: base URL + bearer token + agent.
pub struct HttpTarget {
    base: String,
    token: String,
    agent: ureq::Agent,
}

/// One object registered at finalize. `size` is the LFS (logical) size; the
/// `physical`/`chunks` fields carry the object's post-dedup+compression
/// footprint so the Hub can persist per-object storage stats (0 when unknown).
pub struct FinalizeObject {
    pub oid: String,
    pub size: u64,
    pub physical: u64,
    pub chunks: u64,
}

fn insecure_http_allowed() -> bool {
    matches!(
        std::env::var(ALLOW_INSECURE_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// Bearer token to attach to download GETs against an http(s) Hub, honouring
/// the same plaintext-http gate as uploads: `https://` always gets the token,
/// `http://` only when the dev escape hatch is set (otherwise the GET goes out
/// unauthenticated and a private Hub answers 401 — a clear, safe failure rather
/// than leaking the token in cleartext). Returns `None` for non-http bases (a
/// public CDN / local directory needs no credentials).
pub fn download_auth(base: &str) -> Option<String> {
    let http = base.starts_with("http://");
    let https = base.starts_with("https://");
    if !http && !https {
        return None;
    }
    if http && !insecure_http_allowed() {
        return None;
    }
    find_token().ok()
}

/// Best-effort POST of a transfer report to `{base}/transfers`. Used for the
/// download (pull) benchmark, where there is no finalize call to piggyback on.
/// Silently ignores every error: a failed report must never fail a pull the
/// user already completed successfully.
pub fn report_transfer(base: &str, token: &str, report: &serde_json::Value) {
    let url = format!("{}/transfers", base.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .build();
    let body = match serde_json::to_vec(report) {
        Ok(b) => b,
        Err(_) => return,
    };
    let _ = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_bytes(&body);
}

impl HttpTarget {
    /// Resolve a writable HTTP target for `base`, enforcing the plaintext-HTTP
    /// gate and locating a CAVS access token. Fails fast (before any transfer)
    /// so an unconfigured push does not half-upload.
    pub fn resolve(base: &str) -> Result<Self> {
        if base.starts_with("http://") && !insecure_http_allowed() {
            bail!(
                "refusing to upload over plaintext http ({base}): set {ALLOW_INSECURE_ENV}=1 \
                 for local dev, or use an https:// remote"
            );
        }
        if !base.starts_with("http://") && !base.starts_with("https://") {
            bail!("not an http(s) remote: {base}");
        }
        let token = find_token().context(
            "no CAVS access token: set $CAVS_TOKEN or run `cav login` \
             (token is read from ~/.config/cav/config.toml)",
        )?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(30))
                .build(),
        })
    }

    fn url(&self, rel: &str) -> String {
        format!("{}/{}", self.base, rel.trim_start_matches('/'))
    }

    /// Whether the remote already holds the tree file at `rel`.
    pub fn has(&self, rel: &str) -> Result<bool> {
        let url = self.url(rel);
        match self
            .agent
            .request("HEAD", &url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
        {
            Ok(_) => Ok(true),
            Err(ureq::Error::Status(404 | 410, _)) => Ok(false),
            Err(e) => Err(anyhow::anyhow!("HEAD {url}: {e}")),
        }
    }

    /// Store the tree file at `rel`.
    pub fn put(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        let url = self.url(rel);
        self.agent
            .put(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/octet-stream")
            .send_bytes(bytes)
            .map_err(|e| anyhow::anyhow!("PUT {url}: {e}"))?;
        Ok(())
    }

    /// Register the pushed LFS objects and finalize the push. Returns the new
    /// repository generation reported by the Hub.
    pub fn finalize(
        &self,
        objects: &[FinalizeObject],
        stats: Option<&serde_json::Value>,
    ) -> Result<i64> {
        let url = self.url("finalize");
        let mut payload = serde_json::json!({
            "objects": objects
                .iter()
                .map(|o| serde_json::json!({
                    "oid": o.oid,
                    "size": o.size,
                    "physical": o.physical,
                    "chunks": o.chunks,
                }))
                .collect::<Vec<_>>(),
        });
        // Session-level push benchmark (optional): recorded by the Hub as a
        // transfer event so the dashboard can present upload throughput/dedup.
        if let Some(s) = stats {
            payload["stats"] = s.clone();
        }
        let bytes = serde_json::to_vec(&payload).context("encoding finalize payload")?;
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("POST {url}: {e}"))?;
        let body: serde_json::Value = resp
            .into_string()
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Ok(body.get("generation").and_then(|g| g.as_i64()).unwrap_or(0))
    }

    /// Mirror the whole staging `tree` to the remote: PUT every file the remote
    /// is missing (HEAD-guarded for cross-push wire dedup). Returns (put, skipped).
    pub fn sync_tree(&self, tree: &Path) -> Result<(usize, usize)> {
        let mut files = Vec::new();
        collect_files(tree, tree, &mut files)?;
        let (mut put, mut skipped) = (0usize, 0usize);
        for rel in files {
            // Never publish the private write-lock or the on-disk store; only
            // the static export (assets/, chunks/, meta/, index.json) is served.
            if rel == ".store.lock" || rel.starts_with(".store/") {
                continue;
            }
            if self.has(&rel)? {
                skipped += 1;
                continue;
            }
            let bytes =
                std::fs::read(tree.join(&rel)).with_context(|| format!("reading staged {rel}"))?;
            self.put(&rel, &bytes)?;
            put += 1;
        }
        Ok((put, skipped))
    }
}

/// Recursively collect files under `root`, as paths relative to `root` using
/// forward slashes (the on-wire relative-path convention cavs-fetch expects).
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

/// Locate a CAVS access token: `$CAVS_TOKEN`, then the `token` field of the
/// `cav` CLI config (`$XDG_CONFIG_HOME/cav/config.toml` or
/// `~/.config/cav/config.toml`).
fn find_token() -> Result<String> {
    if let Ok(t) = std::env::var("CAVS_TOKEN") {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    let cfg = config_path().context("cannot locate cav config directory")?;
    let text =
        std::fs::read_to_string(&cfg).with_context(|| format!("reading {}", cfg.display()))?;
    let doc: toml::Value = text.parse().context("parsing cav config.toml")?;
    doc.get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .context("no `token` in cav config")
}

fn config_path() -> Option<std::path::PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(
                std::path::PathBuf::from(xdg)
                    .join("cav")
                    .join("config.toml"),
            );
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(
        std::path::PathBuf::from(home)
            .join(".config")
            .join("cav")
            .join("config.toml"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_never_needs_the_env_gate() {
        // token lookup will fail in CI, but the gate itself must pass for https.
        std::env::remove_var(ALLOW_INSECURE_ENV);
        let err = HttpTarget::resolve("https://hub.example.com/lfs")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            !err.contains("plaintext http"),
            "https must not trip the insecure-http gate, got: {err}"
        );
    }

    #[test]
    fn plaintext_http_is_refused_without_the_env() {
        std::env::remove_var(ALLOW_INSECURE_ENV);
        let err = match HttpTarget::resolve("http://localhost:8080/lfs") {
            Ok(_) => panic!("plaintext http must be refused without the env gate"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("plaintext http"), "got: {err}");
    }
}
