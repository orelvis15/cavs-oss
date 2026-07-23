//! The static tree: an http(s) base URL (Range GETs) or a local directory
//! (slice reads). Any host that serves bytes and honours HTTP Range works —
//! S3, R2, GitHub Pages, nginx, a CDN, or a local folder for offline use.

use anyhow::{Context, Result};
use std::io::Read as _;
use std::path::PathBuf;

pub enum StaticSource {
    Http {
        base: String,
        agent: ureq::Agent,
        /// Optional bearer token sent as `Authorization: Bearer …` on every
        /// request — needed when the static tree is served by an authenticated
        /// host (e.g. the CAVS Node) rather than a public CDN.
        auth: Option<String>,
    },
    Dir(PathBuf),
}

impl StaticSource {
    /// A convenience constructor with a default ureq agent and no auth.
    pub fn new(base: &str) -> Self {
        Self::with_agent(base, ureq::Agent::new())
    }

    /// Interpret `base` as an http(s) URL or a filesystem path (no auth).
    pub fn with_agent(base: &str, agent: ureq::Agent) -> Self {
        if base.starts_with("http://") || base.starts_with("https://") {
            StaticSource::Http {
                base: base.trim_end_matches('/').to_string(),
                agent,
                auth: None,
            }
        } else {
            StaticSource::Dir(PathBuf::from(base))
        }
    }

    /// Like [`Self::new`] but attaches a bearer token to http(s) requests.
    /// `token` is ignored for filesystem bases.
    pub fn with_auth(base: &str, token: Option<String>) -> Self {
        match Self::with_agent(base, ureq::Agent::new()) {
            StaticSource::Http { base, agent, .. } => StaticSource::Http {
                base,
                agent,
                auth: token,
            },
            other => other,
        }
    }

    /// Attach the bearer header to a request when this source carries a token.
    fn authed(auth: &Option<String>, req: ureq::Request) -> ureq::Request {
        match auth {
            Some(t) => req.set("Authorization", &format!("Bearer {t}")),
            None => req,
        }
    }

    /// Like [`Self::get_all`], but a definitive "not there" (HTTP 404/410,
    /// or a missing file) is `Ok(None)` instead of an error, so callers can
    /// negative-cache absence without conflating it with transport failures.
    pub(crate) fn get_all_opt(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        match self {
            StaticSource::Http { base, agent, auth } => {
                let url = format!("{base}/{rel}");
                match Self::authed(auth, agent.get(&url)).call() {
                    Ok(resp) => {
                        let mut out = Vec::new();
                        resp.into_reader()
                            .read_to_end(&mut out)
                            .with_context(|| format!("reading {url}"))?;
                        Ok(Some(out))
                    }
                    Err(ureq::Error::Status(404 | 410, _)) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("GET {url}: {e}")),
                }
            }
            StaticSource::Dir(root) => {
                let path = root.join(rel);
                match std::fs::read(&path) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => {
                        Err(anyhow::Error::new(e).context(format!("reading {}", path.display())))
                    }
                }
            }
        }
    }

    pub(crate) fn get_all(&self, rel: &str) -> Result<Vec<u8>> {
        match self {
            StaticSource::Http { base, agent, auth } => {
                let url = format!("{base}/{rel}");
                let resp = Self::authed(auth, agent.get(&url))
                    .call()
                    .map_err(|e| anyhow::anyhow!("GET {url}: {e}"))?;
                let mut out = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut out)
                    .with_context(|| format!("reading {url}"))?;
                Ok(out)
            }
            StaticSource::Dir(root) => {
                let path = root.join(rel);
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
            }
        }
    }

    pub(crate) fn get_range(&self, rel: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        match self {
            StaticSource::Http { base, agent, auth } => {
                let url = format!("{base}/{rel}");
                let end = offset + len - 1;
                let range = format!("bytes={offset}-{end}");
                let resp = Self::authed(auth, agent.get(&url).set("range", &range))
                    .call()
                    .map_err(|e| anyhow::anyhow!("GET {url} [{range}]: {e}"))?;
                let mut out = Vec::with_capacity(len as usize);
                resp.into_reader()
                    .read_to_end(&mut out)
                    .with_context(|| format!("reading range of {url}"))?;
                // A host that ignored Range returns the whole object; slice.
                if out.len() as u64 > len {
                    let start = offset as usize;
                    let stop = start + len as usize;
                    if stop <= out.len() {
                        return Ok(out[start..stop].to_vec());
                    }
                }
                Ok(out)
            }
            StaticSource::Dir(root) => {
                use std::io::{Seek, SeekFrom};
                let path = root.join(rel);
                let mut f = std::fs::File::open(&path)
                    .with_context(|| format!("opening {}", path.display()))?;
                f.seek(SeekFrom::Start(offset))?;
                let mut out = vec![0u8; len as usize];
                f.read_exact(&mut out)
                    .with_context(|| format!("reading range of {}", path.display()))?;
                Ok(out)
            }
        }
    }
}
