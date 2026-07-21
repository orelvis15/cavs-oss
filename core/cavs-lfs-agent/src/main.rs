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

mod download;
mod protocol;
mod remote;
mod store_sync;
mod upload;

use anyhow::{Context, Result};
use clap::Parser;
use protocol::{Complete, Event, InitResult, ProtoError, ProtoOut, CODE_GENERIC, CODE_NOT_FOUND};
use remote::Remote;
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

    /// Chunking profile for uploads: fastcdc-16k/32k/64k/128k/256k[-n3],
    /// fixed-256k/512k/1m, or auto (per-file by size: <128 MiB -> 16k,
    /// <512 MiB -> 64k, else 128k — tuned from bench/RESULTS.md).
    #[arg(long, default_value = "auto")]
    profile: String,

    /// Compression for uploads: none or zstd-<1..22>.
    #[arg(long, default_value = "zstd-3")]
    compression: String,

    /// Disable the per-chunk BG4 byte-grouping pretransform (on by default
    /// with compression; helps float/int payloads like model weights,
    /// vertex buffers and audio samples).
    #[arg(long)]
    no_bg4: bool,

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

/// Everything resolved at `init` time and shared by all transfers.
struct Session {
    remote: Remote,
    cache_dir: std::path::PathBuf,
    /// Tempdirs of completed downloads: git-lfs consumes the file after our
    /// `complete`, so they must outlive the event — freed on terminate/exit.
    tempdirs: Vec<tempfile::TempDir>,
    /// Lock + open store, created on the first upload and reused for the
    /// whole push session (one store open per push, not per object).
    write: Option<store_sync::WriteSession>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Fail fast on bad session-wide upload config, before speaking protocol.
    let auto = args.profile == "auto";
    let (mode, profile_label) = if auto {
        // Placeholder; auto resolves the profile per file, by size.
        upload::parse_profile("fastcdc-64k")?
    } else {
        upload::parse_profile(&args.profile).context("invalid --profile")?
    };
    let (compress, zstd_level) =
        upload::parse_compression(&args.compression).context("invalid --compression")?;
    let sign_key = args
        .sign_key
        .as_deref()
        .map(upload::load_sign_key)
        .transpose()?;
    let upload_cfg = upload::UploadCfg {
        auto,
        mode,
        profile_label,
        compress,
        zstd_level,
        bg4: !args.no_bg4,
        sign_key,
    };

    let out = ProtoOut::stdout();
    let stdin = std::io::stdin();

    let mut session: Option<Session> = None;

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
                    "[lfs-agent] init: operation={} remote={:?} concurrent={} transfers={}",
                    init.operation, init.remote, init.concurrent, init.concurrenttransfers
                );
                let resolved =
                    remote::resolve(args.remote.as_deref(), &init.remote).and_then(|remote| {
                        let cache_dir = remote::cache_dir(args.cache_dir.as_deref())?;
                        Ok((remote, cache_dir))
                    });
                match resolved {
                    Ok((remote, cache_dir)) => {
                        eprintln!(
                            "[lfs-agent] remote: {remote:?}, cache: {}",
                            cache_dir.display()
                        );
                        session = Some(Session {
                            remote,
                            cache_dir,
                            tempdirs: Vec::new(),
                            write: None,
                        });
                        out.send(&InitResult::default());
                    }
                    Err(e) => {
                        out.send(&InitResult {
                            error: Some(ProtoError::new(protocol::CODE_INIT, format!("{e:#}"))),
                        });
                        anyhow::bail!("init failed: {e:#}");
                    }
                }
            }
            Event::Download(dl) => {
                let Some(session) = session.as_mut() else {
                    out.send(&Complete::err(
                        &dl.oid,
                        ProtoError::new(CODE_GENERIC, "protocol error: download before init"),
                    ));
                    continue;
                };
                // On a directory remote a missing asset is a clean 404;
                // over HTTP the fetch itself reports the missing manifest.
                if let Remote::Dir { tree, .. } = &session.remote {
                    if !download::exists_at_dir_remote(tree, &dl.oid) {
                        out.send(&Complete::err(
                            &dl.oid,
                            ProtoError::new(CODE_NOT_FOUND, "object not found at remote"),
                        ));
                        continue;
                    }
                }
                let tmp_root = session.cache_dir.join("tmp");
                match download::handle(
                    &session.remote.fetch_base(),
                    &dl.oid,
                    &session.cache_dir,
                    &tmp_root,
                    args.connections,
                    args.pubkey.as_deref(),
                    &out,
                ) {
                    Ok((path, tmpdir)) => {
                        out.send(&Complete::ok_download(&dl.oid, &path));
                        session.tempdirs.push(tmpdir);
                    }
                    Err(e) => {
                        eprintln!("[lfs-agent] download {} failed: {e:#}", dl.oid);
                        out.send(&Complete::err(
                            &dl.oid,
                            ProtoError::new(CODE_GENERIC, format!("{e:#}")),
                        ));
                    }
                }
            }
            Event::Upload(ul) => {
                let Some(session) = session.as_mut() else {
                    out.send(&Complete::err(
                        &ul.oid,
                        ProtoError::new(CODE_GENERIC, "protocol error: upload before init"),
                    ));
                    continue;
                };
                let Remote::Dir { tree, store } = &session.remote else {
                    out.send(&Complete::err(
                        &ul.oid,
                        ProtoError::new(
                            CODE_GENERIC,
                            "remote is read-only (http); uploads need a directory remote",
                        ),
                    ));
                    continue;
                };
                // Lock + open the store once per push session, lazily.
                let write = match &mut session.write {
                    Some(w) => w,
                    None => match store_sync::open_session(tree, store) {
                        Ok(w) => session.write.insert(w),
                        Err(e) => {
                            eprintln!("[lfs-agent] cannot open store: {e:#}");
                            out.send(&Complete::err(
                                &ul.oid,
                                ProtoError::new(CODE_GENERIC, format!("{e:#}")),
                            ));
                            continue;
                        }
                    },
                };
                match upload::handle(write, &ul.oid, &ul.path, ul.size, &upload_cfg, &out) {
                    Ok(()) => out.send(&Complete::ok_upload(&ul.oid)),
                    Err(e) => {
                        eprintln!("[lfs-agent] upload {} failed: {e:#}", ul.oid);
                        out.send(&Complete::err(
                            &ul.oid,
                            ProtoError::new(CODE_GENERIC, format!("{e:#}")),
                        ));
                    }
                }
            }
            Event::Terminate => {
                eprintln!("[lfs-agent] terminate");
                break;
            }
        }
    }

    // Session finalize (Xet-style): commit the batched publishes and export
    // every uploaded asset into the static tree, once per push. Reached on
    // terminate and on EOF; a crash before this point publishes nothing, so
    // the next push simply re-ingests. An error here is fatal — the push
    // must not look successful with nothing published.
    if let Some(session) = session.as_mut() {
        if let Some(write) = session.write.as_mut() {
            let n = write.pending_exports.len();
            write
                .finalize()
                .with_context(|| format!("finalizing push session ({n} assets)"))?;
            if n > 0 {
                eprintln!("[lfs-agent] finalize: published {n} assets");
            }
        }
    }
    Ok(())
}
