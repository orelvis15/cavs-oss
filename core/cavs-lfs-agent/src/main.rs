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
mod http_push;
mod protocol;
mod remote;
mod store_sync;
mod upload;

use anyhow::{Context, Result};
use clap::Parser;
use protocol::{Complete, Event, InitResult, ProtoError, ProtoOut, CODE_GENERIC, CODE_NOT_FOUND};
use remote::Remote;
use std::io::BufRead;
use std::time::Instant;

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

    /// Parallel connections for downloads: 0 = adaptive (AIMD, grows and
    /// shrinks with observed pressure), N = a fixed pool of N connections.
    #[arg(long, default_value_t = 0)]
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
    /// Writable HTTP remote (resolved at init for upload sessions). When set,
    /// uploads ingest into a local staging mirror and the export is pushed to
    /// the Hub at finalize.
    http: Option<http_push::HttpTarget>,
    /// (oid, size) of every object seen this push, registered on the Hub at
    /// finalize so the repository reflects the push.
    uploaded: Vec<(String, u64)>,
    /// Session-wide metadata resolver: L1/L2 caches + meta-pack prefetch
    /// shared across every download of this process.
    resolver: cavs_fetch::MetadataResolver,
    /// Aggregate download stats for the terminate-time breakdown.
    downloads: u64,
    agg: cavs_fetch::FetchStats,
    /// Upload benchmark accumulators (faithful, measured — reported to the Hub
    /// at finalize). `up_started` is set on the first upload so the push wall
    /// time excludes pre-push idle.
    up_started: Option<Instant>,
    up_logical: u64,
    up_new_bytes: u64,
    up_chunks: u64,
    up_new_chunks: u64,
    up_ingest_ms: u64,
    up_objects: u64,
}

impl Session {
    fn add_download(&mut self, stats: &cavs_fetch::FetchStats) {
        self.downloads += 1;
        let a = &mut self.agg;
        a.wire_bytes += stats.wire_bytes;
        a.raw_bytes += stats.raw_bytes;
        a.fetched += stats.fetched;
        a.reused += stats.reused;
        a.logical_bytes += stats.logical_bytes;
        a.requests += stats.requests;
        a.useful_bytes += stats.useful_bytes;
        a.selective_retries += stats.selective_retries;
        a.throttle_waits += stats.throttle_waits;
        a.metadata_requests += stats.metadata_requests;
        a.metadata_ms += stats.metadata_ms;
        a.plan_ms += stats.plan_ms;
        a.payload_ms += stats.payload_ms;
        a.reconstruct_ms += stats.reconstruct_ms;
    }

    /// The Round 3A phase breakdown: where a many-object session actually
    /// spent its time, so metadata cost is visible instead of folded into
    /// one opaque wall time.
    fn print_summary(&self) {
        if self.downloads == 0 {
            return;
        }
        let m = self.resolver.stats();
        eprintln!(
            "[lfs-agent] session: {} downloads | metadata {} req / {} ms \
             (l1 {} l2 {} packs {} prefetched {} fallbacks {}) | \
             payload {} req / {} ms | plan {} ms | reconstruct {} ms | \
             {} wire bytes / {} useful",
            self.downloads,
            self.agg.metadata_requests,
            self.agg.metadata_ms,
            m.l1_hits,
            m.l2_hits,
            m.pack_fetches,
            m.prefetched,
            m.fallback_singles,
            self.agg.requests,
            self.agg.payload_ms,
            self.agg.plan_ms,
            self.agg.reconstruct_ms,
            self.agg.wire_bytes,
            self.agg.useful_bytes,
        );
    }
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
                        // For an upload against an http(s) remote, resolve the
                        // writable Hub target now (enforces the plaintext-http
                        // gate + token lookup) so a misconfigured push fails at
                        // init instead of after transferring bytes.
                        let http = if init.operation == "upload" {
                            if let Remote::Http(base) = &remote {
                                match http_push::HttpTarget::resolve(base) {
                                    Ok(t) => Some(t),
                                    Err(e) => {
                                        out.send(&InitResult {
                                            error: Some(ProtoError::new(
                                                protocol::CODE_INIT,
                                                format!("{e:#}"),
                                            )),
                                        });
                                        anyhow::bail!("init failed: {e:#}");
                                    }
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let resolver = cavs_fetch::MetadataResolver::new(&cache_dir);
                        session = Some(Session {
                            remote,
                            cache_dir,
                            tempdirs: Vec::new(),
                            write: None,
                            http,
                            uploaded: Vec::new(),
                            resolver,
                            downloads: 0,
                            agg: cavs_fetch::FetchStats::default(),
                            up_started: None,
                            up_logical: 0,
                            up_new_bytes: 0,
                            up_chunks: 0,
                            up_new_chunks: 0,
                            up_ingest_ms: 0,
                            up_objects: 0,
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
                    &session.resolver,
                    &out,
                ) {
                    Ok((path, tmpdir, stats)) => {
                        session.add_download(&stats);
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
                // Where the ingest/export lands: a directory remote writes its
                // static tree in place; an http remote stages into a local
                // mirror (under the cache dir) that is pushed to the Hub at
                // finalize. Both paths share the same ingest/pack/export code.
                let (tree, store) = match &session.remote {
                    Remote::Dir { tree, store } => (tree.clone(), store.clone()),
                    Remote::Http(_) => {
                        let tree = session.cache_dir.join("mirror");
                        let store = tree.join(".store");
                        (tree, store)
                    }
                };
                // Lock + open the store once per push session, lazily.
                let write = match &mut session.write {
                    Some(w) => w,
                    None => match store_sync::open_session(&tree, &store) {
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
                if session.up_started.is_none() {
                    session.up_started = Some(Instant::now());
                }
                let t0 = Instant::now();
                match upload::handle(write, &ul.oid, &ul.path, ul.size, &upload_cfg, &out) {
                    Ok(stats) => {
                        session.up_ingest_ms += t0.elapsed().as_millis() as u64;
                        session.up_logical += ul.size;
                        session.up_new_bytes += stats.new_bytes;
                        session.up_chunks += stats.chunks;
                        session.up_new_chunks += stats.new_chunks;
                        session.up_objects += 1;
                        session.uploaded.push((ul.oid.clone(), ul.size));
                        out.send(&Complete::ok_upload(&ul.oid));
                    }
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
        session.print_summary();
        if let Some(write) = session.write.as_mut() {
            let n = write.pending_exports.len();
            write
                .finalize()
                .with_context(|| format!("finalizing push session ({n} assets)"))?;
            if n > 0 {
                eprintln!("[lfs-agent] finalize: published {n} assets");
            }
        }
        // http remote: mirror the freshly exported static tree to the Hub and
        // register the push. Only runs when this was an upload session against
        // an http(s) remote (see init).
        if let Some(http) = session.http.as_ref() {
            let tree = session.cache_dir.join("mirror");
            if tree.is_dir() {
                let t_mirror = Instant::now();
                let (put, skipped) = http
                    .sync_tree(&tree)
                    .context("mirroring static export to the hub")?;
                let mirror_ms = t_mirror.elapsed().as_millis() as u64;
                // Attach each object's post-dedup+compression footprint (looked
                // up from the now-finalized store) so the Hub can persist
                // per-object storage stats. 0 when the asset isn't resolvable.
                let store = session.write.as_ref().map(|w| &w.store);
                let objects: Vec<http_push::FinalizeObject> = session
                    .uploaded
                    .iter()
                    .map(|(oid, size)| {
                        let (physical, chunks) = store
                            .and_then(|s| s.asset_stored_stats(oid))
                            .unwrap_or((0, 0));
                        http_push::FinalizeObject {
                            oid: oid.clone(),
                            size: *size,
                            physical,
                            chunks,
                        }
                    })
                    .collect();
                // Faithful, measured push benchmark. Times are wall-clock of the
                // agent's own work (ingest + mirror + total since first upload);
                // bytes/chunks are exact counters from the store, not estimates.
                let total_ms = session
                    .up_started
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                let stats = serde_json::json!({
                    "kind": "push",
                    "duration_ms": total_ms,
                    "ingest_ms": session.up_ingest_ms,
                    "mirror_ms": mirror_ms,
                    "logical_bytes": session.up_logical,
                    "stored_bytes": session.up_new_bytes,
                    "object_count": session.up_objects,
                    "chunk_count": session.up_chunks,
                    "new_chunks": session.up_new_chunks,
                    "reused_chunks": session.up_chunks.saturating_sub(session.up_new_chunks),
                    "files_put": put as u64,
                    "files_skipped": skipped as u64,
                });
                let gen = http
                    .finalize(&objects, Some(&stats))
                    .context("finalizing push on the hub")?;
                eprintln!(
                    "[lfs-agent] hub: pushed {put} files ({skipped} already present), \
                     {} objects, generation {gen}",
                    session.uploaded.len()
                );
            }
        }

        // Download session over an http(s) Hub: report the faithful fetch stats
        // so the dashboard can show pull benchmarks. Best-effort — a failed
        // report must never fail the pull the user already completed.
        if session.downloads > 0 {
            let base = session.remote.fetch_base();
            if let Some(token) = http_push::download_auth(&base) {
                let m = session.resolver.stats();
                let a = &session.agg;
                let report = serde_json::json!({
                    "kind": "pull",
                    "duration_ms": a.metadata_ms + a.plan_ms + a.payload_ms + a.reconstruct_ms,
                    "object_count": session.downloads,
                    "logical_bytes": a.logical_bytes,
                    "wire_bytes": a.wire_bytes,
                    "useful_bytes": a.useful_bytes,
                    "chunk_count": a.fetched + a.reused,
                    "new_chunks": a.fetched,
                    "reused_chunks": a.reused,
                    "requests": a.requests,
                    "metadata_ms": a.metadata_ms,
                    "plan_ms": a.plan_ms,
                    "payload_ms": a.payload_ms,
                    "reconstruct_ms": a.reconstruct_ms,
                    "metadata_requests": a.metadata_requests,
                    "cache_l1_hits": m.l1_hits,
                    "cache_l2_hits": m.l2_hits,
                });
                http_push::report_transfer(&base, &token, &report);
            }
        }
    }
    Ok(())
}
