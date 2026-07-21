//! `cavs-lfs-agent` — a Git LFS *standalone* custom transfer agent backed by
//! CAVS: files are content-defined-chunked, zstd-compressed and stored in a
//! content-addressable store, so only the chunks that actually changed
//! travel (and are stored) across versions of a large file.
//!
//! Setup in a repository:
//! ```sh
//! git config lfs.standalonetransferagent cavs
//! git config lfs.customtransfer.cavs.path /path/to/cavs-lfs-agent
//! git config lfs.customtransfer.cavs.concurrent false
//! # optional: git config lfs.customtransfer.cavs.args "--remote /srv/lfs-cavs"
//! ```
//!
//! The agent speaks NDJSON on stdin/stdout (`init`/`download`/`upload`/
//! `terminate`); diagnostics go to stderr only.

mod protocol;

use anyhow::Result;
use clap::Parser;
use protocol::{Complete, Event, InitResult, ProtoError, ProtoOut, CODE_GENERIC};
use std::io::BufRead;

#[derive(Debug, Parser)]
#[command(name = "cavs-lfs-agent", version, about)]
struct Args {
    /// Remote override: a directory path, file:// URL (read/write) or
    /// http(s):// base URL (read-only). Falls back to $CAVS_LFS_REMOTE,
    /// then to the remote announced by git-lfs in the init event.
    #[arg(long)]
    remote: Option<String>,

    /// Chunk cache directory. Falls back to $CAVS_LFS_CACHE, then to
    /// `<git-dir>/lfs/cavs/cache`, then to `~/.cache/cavs-lfs-agent`.
    #[arg(long)]
    cache_dir: Option<std::path::PathBuf>,

    /// Chunking profile for uploads (fastcdc-16k/32k/64k/128k[-n3],
    /// fixed-256k/512k/1m, or auto).
    #[arg(long, default_value = "auto")]
    profile: String,

    /// Compression for uploads: none or zstd-<1..22>.
    #[arg(long, default_value = "zstd-3")]
    compression: String,

    /// Require signed manifests on download: 64-hex Ed25519 public key.
    #[arg(long)]
    pubkey: Option<String>,

    /// Sign uploads: path to a 32-byte Ed25519 secret key file (raw or hex).
    #[arg(long)]
    sign_key: Option<std::path::PathBuf>,

    /// Parallel connections for downloads.
    #[arg(long, default_value_t = 8)]
    connections: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let out = ProtoOut::stdout();
    let stdin = std::io::stdin();

    let mut session: Option<protocol::InitEvent> = None;

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event = match protocol::read_event(&line) {
            Ok(e) => e,
            Err(e) => {
                // A malformed protocol stream is fatal: we can no longer
                // trust our position in the dialogue.
                eprintln!("[lfs-agent] fatal: cannot parse event: {e}: {line}");
                anyhow::bail!("malformed protocol event");
            }
        };
        match event {
            Event::Init(init) => {
                eprintln!(
                    "[lfs-agent] init: operation={} remote={:?}",
                    init.operation, init.remote
                );
                session = Some(init);
                out.send(&InitResult::default());
            }
            Event::Download(dl) => {
                let _ = &session;
                out.send(&Complete::err(
                    &dl.oid,
                    ProtoError::new(CODE_GENERIC, "download not implemented yet"),
                ));
            }
            Event::Upload(ul) => {
                out.send(&Complete::err(
                    &ul.oid,
                    ProtoError::new(CODE_GENERIC, "upload not implemented yet"),
                ));
            }
            Event::Terminate => {
                eprintln!("[lfs-agent] terminate");
                break;
            }
        }
    }
    let _ = args;
    Ok(())
}
