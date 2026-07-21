//! The Git LFS custom transfer protocol: newline-delimited JSON over
//! stdin/stdout (see git-lfs `docs/custom-transfers.md`).
//!
//! git-lfs sends `init`, then a sequence of `download`/`upload` events (one
//! at a time — the dialogue within a single agent process is strictly
//! sequential), then `terminate`. The agent replies `{}` to `init`, emits
//! optional `progress` events, and exactly one `complete` per object.
//!
//! stdout carries ONLY protocol JSON; all diagnostics go to stderr.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Generic per-object failure.
pub const CODE_GENERIC: i32 = 1;
/// The object is not present at the remote.
pub const CODE_NOT_FOUND: i32 = 404;
/// The session could not be initialised (bad/unresolvable remote, ...).
pub const CODE_INIT: i32 = 32;

/// Events received from git-lfs on stdin.
///
/// Deserialization is deliberately tolerant: unknown fields are ignored and
/// optional fields default, so minor protocol drift across git-lfs versions
/// does not break the agent.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    Init(InitEvent),
    Upload(UploadEvent),
    Download(DownloadEvent),
    Terminate,
}

#[derive(Debug, Default, Deserialize)]
pub struct InitEvent {
    /// `"download"` or `"upload"` — fixed for the whole session.
    #[serde(default)]
    pub operation: String,
    /// Remote name (e.g. `origin`) or URL, as given to git-lfs.
    #[serde(default)]
    pub remote: String,
    #[serde(default)]
    pub concurrent: bool,
    #[serde(default)]
    pub concurrenttransfers: u32,
}

#[derive(Debug, Deserialize)]
pub struct UploadEvent {
    /// LFS object id: lowercase sha256 hex of the content.
    pub oid: String,
    #[serde(default)]
    pub size: u64,
    /// Absolute path of the local file to upload.
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct DownloadEvent {
    pub oid: String,
    #[serde(default)]
    pub size: u64,
}

/// `{"code":…,"message":…}` payload used by init and complete errors.
#[derive(Debug, Serialize)]
pub struct ProtoError {
    pub code: i32,
    pub message: String,
}

impl ProtoError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Reply to `init`: `{}` on success, `{"error":{…}}` on failure.
#[derive(Debug, Default, Serialize)]
pub struct InitResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtoError>,
}

#[derive(Debug, Serialize)]
pub struct Progress<'a> {
    pub event: &'static str,
    pub oid: &'a str,
    #[serde(rename = "bytesSoFar")]
    pub bytes_so_far: u64,
    #[serde(rename = "bytesSinceLast")]
    pub bytes_since_last: u64,
}

impl<'a> Progress<'a> {
    pub fn new(oid: &'a str, bytes_so_far: u64, bytes_since_last: u64) -> Self {
        Self {
            event: "progress",
            oid,
            bytes_so_far,
            bytes_since_last,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Complete<'a> {
    pub event: &'static str,
    pub oid: &'a str,
    /// Download success: where git-lfs should pick the file up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<&'a Path>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtoError>,
}

impl<'a> Complete<'a> {
    pub fn ok_upload(oid: &'a str) -> Self {
        Self {
            event: "complete",
            oid,
            path: None,
            error: None,
        }
    }

    pub fn ok_download(oid: &'a str, path: &'a Path) -> Self {
        Self {
            event: "complete",
            oid,
            path: Some(path),
            error: None,
        }
    }

    pub fn err(oid: &'a str, error: ProtoError) -> Self {
        Self {
            event: "complete",
            oid,
            path: None,
            error: Some(error),
        }
    }
}

/// Serialized writer for protocol events: one JSON object per line, flushed
/// immediately. A single shared instance keeps the one-writer discipline —
/// progress callbacks fire from fetch worker threads.
pub struct ProtoOut {
    out: Mutex<std::io::Stdout>,
}

impl ProtoOut {
    pub fn stdout() -> Self {
        Self {
            out: Mutex::new(std::io::stdout()),
        }
    }

    pub fn send(&self, msg: &impl Serialize) {
        let line = serde_json::to_string(msg).expect("protocol events always serialize");
        let mut out = self.out.lock().unwrap();
        // A broken pipe here means git-lfs is gone; nothing sensible left to
        // do but let the next stdin read observe EOF and exit.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// Parse one NDJSON line from git-lfs.
pub fn read_event(line: &str) -> Result<Event, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init() {
        let e = read_event(
            r#"{"event":"init","operation":"download","remote":"origin","concurrent":true,"concurrenttransfers":3}"#,
        )
        .unwrap();
        match e {
            Event::Init(i) => {
                assert_eq!(i.operation, "download");
                assert_eq!(i.remote, "origin");
                assert!(i.concurrent);
                assert_eq!(i.concurrenttransfers, 3);
            }
            other => panic!("expected init, got {other:?}"),
        }
    }

    #[test]
    fn parses_download_without_action_or_size() {
        // Standalone mode omits `action`; tolerate missing `size` too.
        let e = read_event(r#"{"event":"download","oid":"abc123","action":null}"#).unwrap();
        match e {
            Event::Download(d) => {
                assert_eq!(d.oid, "abc123");
                assert_eq!(d.size, 0);
            }
            other => panic!("expected download, got {other:?}"),
        }
    }

    #[test]
    fn parses_upload_with_unknown_fields() {
        let e = read_event(
            r#"{"event":"upload","oid":"def","size":7,"path":"/tmp/f","action":{"href":"x"},"future":1}"#,
        )
        .unwrap();
        match e {
            Event::Upload(u) => {
                assert_eq!(u.oid, "def");
                assert_eq!(u.size, 7);
                assert_eq!(u.path, PathBuf::from("/tmp/f"));
            }
            other => panic!("expected upload, got {other:?}"),
        }
    }

    #[test]
    fn parses_terminate() {
        assert!(matches!(
            read_event(r#"{"event":"terminate"}"#).unwrap(),
            Event::Terminate
        ));
    }

    #[test]
    fn init_result_shapes() {
        assert_eq!(serde_json::to_string(&InitResult::default()).unwrap(), "{}");
        let err = InitResult {
            error: Some(ProtoError::new(CODE_INIT, "no remote")),
        };
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"error":{"code":32,"message":"no remote"}}"#
        );
    }

    #[test]
    fn complete_shapes() {
        let ok = Complete::ok_download("abc", Path::new("/tmp/abc"));
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"event":"complete","oid":"abc","path":"/tmp/abc"}"#
        );
        let ok = Complete::ok_upload("abc");
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"event":"complete","oid":"abc"}"#
        );
        let err = Complete::err("abc", ProtoError::new(CODE_NOT_FOUND, "missing"));
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"event":"complete","oid":"abc","error":{"code":404,"message":"missing"}}"#
        );
    }

    #[test]
    fn progress_shape() {
        let p = Progress::new("abc", 100, 40);
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"event":"progress","oid":"abc","bytesSoFar":100,"bytesSinceLast":40}"#
        );
    }
}
