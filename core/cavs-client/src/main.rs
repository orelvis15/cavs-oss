//! `cavs-client` — native CAVS-1 streaming client with a persistent
//! content-addressable cache.
//!
//! On fetch it announces its have-set to the origin, receives inline/ref
//! plans, resolves references from the local cache, verifies every chunk by
//! BLAKE3, reconstructs playable outputs and reports real egress savings.

mod cache;

use anyhow::{bail, Context, Result};
use cache::ChunkCache;
use cavs_hash::to_hex;
use cavs_proto::{BatchRequest, DeliveryInstr, Manifest, SessionOpenRequest, SessionOpenResponse};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

/// Segments requested per batch round-trip.
const BATCH_SIZE: usize = 64;
/// Above this many cached chunks, summarise the have-set with a Bloom filter
/// instead of an exact hash list (keeps the session-open body compact).
const BLOOM_THRESHOLD: usize = 256;

#[derive(Parser)]
#[command(name = "cavs-client", version, about = "CAVS-1 streaming client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List assets available on a server.
    List {
        server: String,
        /// Trust this PEM certificate (e.g. a self-signed dev cert).
        #[arg(long)]
        ca: Option<PathBuf>,
    },
    /// Fetch an asset, reconstruct it under -o, and print egress stats.
    Fetch {
        /// Server base URL, e.g. http://127.0.0.1:8990
        server: String,
        /// Asset name as listed by the server.
        asset: String,
        /// Output directory.
        #[arg(short, long)]
        output: PathBuf,
        /// Persistent chunk cache directory (survives across fetches).
        #[arg(long, default_value = ".cavs-cache")]
        cache: PathBuf,
        /// Write exact fetch statistics as JSON to this path.
        #[arg(long)]
        stats_json: Option<PathBuf>,
        /// Trust this PEM certificate (e.g. a self-signed dev cert).
        #[arg(long)]
        ca: Option<PathBuf>,
        /// Require the asset to be signed by this Ed25519 public key
        /// (64 hex chars, or a path to a .pub file).
        #[arg(long)]
        pubkey: Option<String>,
    },
    /// Fetch to a temp dir and play the first video track with ffplay.
    Play {
        server: String,
        asset: String,
        #[arg(long, default_value = ".cavs-cache")]
        cache: PathBuf,
        #[arg(long)]
        ca: Option<PathBuf>,
        #[arg(long)]
        pubkey: Option<String>,
    },
}

fn main() -> Result<()> {
    // Pick a rustls crypto provider explicitly (see cavs-server note).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    match cli.command {
        Command::List { server, ca } => {
            let agent = build_agent(ca.as_deref())?;
            let body = http_get_string(&agent, &format!("{server}/api/assets"))?;
            let assets: Vec<cavs_proto::AssetSummary> = serde_json::from_str(&body)?;
            for a in assets {
                println!(
                    "{}  tracks={} segments={} chunks={}",
                    a.name, a.tracks, a.segments, a.unique_chunks
                );
            }
            Ok(())
        }
        Command::Fetch {
            server,
            asset,
            output,
            cache,
            stats_json,
            ca,
            pubkey,
        } => {
            let agent = build_agent(ca.as_deref())?;
            let (_, stats) = fetch(&agent, &server, &asset, &output, &cache, pubkey.as_deref())?;
            if let Some(path) = stats_json {
                std::fs::write(&path, stats.to_json())
                    .with_context(|| format!("cannot write {}", path.display()))?;
            }
            Ok(())
        }
        Command::Play {
            server,
            asset,
            cache,
            ca,
            pubkey,
        } => {
            let agent = build_agent(ca.as_deref())?;
            let tmp = tempfile::tempdir()?;
            let (primaries, _) = fetch(
                &agent,
                &server,
                &asset,
                tmp.path(),
                &cache,
                pubkey.as_deref(),
            )?;
            let Some(target) = primaries.first() else {
                bail!("no playable track in asset {asset}");
            };
            eprintln!("[play] launching ffplay on {}", target.display());
            let status = std::process::Command::new("ffplay")
                .args(["-hide_banner", "-loglevel", "error", "-autoexit"])
                .arg(target)
                .status()
                .context("failed to spawn ffplay (is it installed?)")?;
            if !status.success() {
                bail!("ffplay exited with an error");
            }
            Ok(())
        }
    }
}

/// Exact fetch statistics, exportable as JSON for benchmarking.
/// `inline_bytes` counts wire payload bytes (as transmitted, possibly
/// compressed); `inline_raw_bytes` counts the same payloads uncompressed.
pub struct FetchStats {
    pub inline_bytes: u64,
    pub inline_raw_bytes: u64,
    pub inline_chunks: u64,
    pub refs: u64,
    pub logical_bytes: u64,
    /// Route taken: "chunks", "references" or "bootstrap" (v2 dual route).
    pub delivery_mode: &'static str,
    /// Chunks inserted into the cache by slicing the bootstrap artifact.
    pub seeded_chunks: u64,
    /// Time spent seeding the cache from the bootstrap, in ms.
    pub seed_ms: u64,
}

impl FetchStats {
    fn to_json(&self) -> String {
        format!(
            "{{\"inline_bytes\":{},\"inline_raw_bytes\":{},\"inline_chunks\":{},\"refs\":{},\"logical_bytes\":{},\"delivery_mode\":\"{}\",\"seeded_chunks\":{},\"seed_ms\":{}}}",
            self.inline_bytes,
            self.inline_raw_bytes,
            self.inline_chunks,
            self.refs,
            self.logical_bytes,
            self.delivery_mode,
            self.seeded_chunks,
            self.seed_ms
        )
    }
}

/// HTTP agent; with `--ca`, trusts exactly that PEM certificate (dev TLS).
fn build_agent(ca: Option<&Path>) -> Result<ureq::Agent> {
    let Some(ca_path) = ca else {
        return Ok(ureq::AgentBuilder::new().build());
    };
    let pem = std::fs::File::open(ca_path)
        .with_context(|| format!("cannot open CA file {}", ca_path.display()))?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut std::io::BufReader::new(pem)) {
        roots
            .add(cert.context("reading certificate")?)
            .context("adding certificate to trust store")?;
    }
    if roots.is_empty() {
        anyhow::bail!("{} contains no certificates", ca_path.display());
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(ureq::AgentBuilder::new()
        .tls_config(std::sync::Arc::new(config))
        .build())
}

/// Verify the manifest's Ed25519 content signature against a trusted key.
/// Checks: signer matches, Merkle root recomputes from the chunk table, the
/// signature verifies, and every referenced chunk is covered by the table.
fn verify_manifest_signature(manifest: &Manifest, trusted_pubkey_hex: &str) -> Result<()> {
    let sig_hex = manifest
        .signature
        .as_deref()
        .context("asset is not signed but --pubkey was given")?;
    let signer_hex = manifest
        .signer_pubkey
        .as_deref()
        .context("asset signature has no public key")?;
    if !signer_hex.eq_ignore_ascii_case(trusted_pubkey_hex) {
        anyhow::bail!(
            "asset is signed by an untrusted key: {signer_hex} (expected {trusted_pubkey_hex})"
        );
    }

    let leaves: Vec<cavs_hash::ChunkHash> = manifest
        .chunk_table
        .iter()
        .map(|h| cavs_hash::from_hex(h).context("bad hash in chunk_table"))
        .collect::<Result<_>>()?;
    let root = cavs_hash::merkle_root(&leaves);
    if !manifest.merkle_root.eq_ignore_ascii_case(&to_hex(&root)) {
        anyhow::bail!("manifest merkle_root does not match its chunk_table");
    }

    let pk_bytes: [u8; 32] = decode_hex(signer_hex, 32)?.try_into().unwrap();
    let sig_bytes: [u8; 64] = decode_hex(sig_hex, 64)?.try_into().unwrap();
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes).context("invalid signer public key")?;
    let message = cavs_hash::content_signature_message(&root, leaves.len() as u64);
    use ed25519_dalek::Verifier;
    key.verify(&message, &ed25519_dalek::Signature::from_bytes(&sig_bytes))
        .map_err(|_| anyhow::anyhow!("content signature is INVALID"))?;

    // Every chunk the manifest references must be covered by the signed table.
    let table: std::collections::HashSet<&str> =
        manifest.chunk_table.iter().map(|s| s.as_str()).collect();
    for hash in manifest_chunk_hashes(manifest) {
        if !table.contains(hash.as_str()) {
            anyhow::bail!("chunk {hash} referenced but not covered by the signed table");
        }
    }
    Ok(())
}

fn decode_hex(s: &str, len: usize) -> Result<Vec<u8>> {
    if s.len() != len * 2 {
        anyhow::bail!("expected {} hex chars, got {}", len * 2, s.len());
    }
    (0..len)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("bad hex"))
        .collect()
}

fn fetch(
    agent: &ureq::Agent,
    server: &str,
    asset: &str,
    output: &Path,
    cache_dir: &Path,
    pubkey: Option<&str>,
) -> Result<(Vec<PathBuf>, FetchStats)> {
    let cache = ChunkCache::open(cache_dir)?;

    // 1. Manifest (+ optional signature enforcement).
    let manifest_json = http_get_string(agent, &format!("{server}/api/assets/{asset}/manifest"))?;
    let manifest: Manifest = serde_json::from_str(&manifest_json)?;
    if let Some(pk) = pubkey {
        // Accept a literal hex key or a path to a .pub file.
        let pk_hex = if pk.len() == 64 && pk.chars().all(|c| c.is_ascii_hexdigit()) {
            pk.to_string()
        } else {
            std::fs::read_to_string(pk)
                .with_context(|| format!("cannot read pubkey file {pk}"))?
                .trim()
                .to_string()
        };
        verify_manifest_signature(&manifest, &pk_hex)?;
        eprintln!("[fetch] content signature OK (signer {})", &pk_hex[..16]);
    }

    // 2. Announce our have-set (intersecting locally with the manifest keeps
    //    the request small: only hashes this asset actually uses). Large
    //    have-sets are summarised with a Bloom filter so the session-open
    //    body stays compact; false positives are repaired in step 3b.
    let have: Vec<String> = manifest_chunk_hashes(&manifest)
        .into_iter()
        .filter(|h| cache.contains(h))
        .collect();
    let open_req = if have.len() > BLOOM_THRESHOLD {
        let mut bloom = cavs_proto::BloomFilter::with_capacity(have.len());
        for hex in &have {
            if let Some(h) = cavs_hash::from_hex(hex) {
                bloom.insert(&h);
            }
        }
        serde_json::to_string(&SessionOpenRequest {
            have: Vec::new(),
            have_bloom: Some(bloom),
        })?
    } else {
        serde_json::to_string(&SessionOpenRequest {
            have: have.clone(),
            have_bloom: None,
        })?
    };
    let session: SessionOpenResponse = serde_json::from_str(&http_post_json(
        agent,
        &format!("{server}/api/assets/{asset}/sessions"),
        &open_req,
    )?)?;
    eprintln!(
        "[fetch] session {} (server matched {} cached chunks)",
        session.session_id, session.known_chunks
    );

    // v2 dual route: for a cold cache the server may have measured that the
    // full compressed artifact is cheaper than the chunk path. Download it,
    // verify, install, and seed the local chunk cache from it — so the NEXT
    // fetch (an update) pays only for what changed. Any failure falls back
    // to the normal chunk path below.
    if session.delivery_mode.as_deref() == Some(cavs_proto::DELIVERY_BOOTSTRAP) {
        match fetch_bootstrap(agent, server, asset, &manifest, &session, &cache, output) {
            Ok(result) => return Ok(result),
            Err(e) => {
                eprintln!("[fetch] bootstrap route failed ({e}); falling back to chunks")
            }
        }
    }

    // 3. Batches, processed as a stream: each inline chunk is verified and
    //    lands in the disk cache as it arrives — nothing accumulates in RAM
    //    (the content-addressable cache IS the store). References are only
    //    counted here; reconstruction reads them from the cache.
    let mut inline_bytes = 0u64;
    let mut inline_raw_bytes = 0u64;
    let mut inline_count = 0u64;
    let mut ref_count = 0u64;
    // Refs the server assumed we had (bloom false positives) but our cache
    // actually lacks — repaired after the batch loop.
    let mut missing_refs: Vec<cavs_hash::ChunkHash> = Vec::new();

    let all_tracks: Vec<u32> = manifest.tracks.iter().map(|t| t.track_id).collect();
    let mut segment_ids: Vec<u64> = manifest.segments.iter().map(|s| s.segment_id).collect();
    segment_ids.sort_unstable();

    let mut first = true;
    for group in segment_ids.chunks(BATCH_SIZE.max(1)) {
        let req = BatchRequest {
            track_inits: if first {
                all_tracks.clone()
            } else {
                Vec::new()
            },
            segment_ids: group.to_vec(),
        };
        first = false;
        let mut reader = http_post_reader(
            agent,
            &format!("{server}/api/sessions/{}/batch", session.session_id),
            &serde_json::to_string(&req)?,
        )?;
        cavs_proto::decode_stream(&mut reader, |item| {
            let cavs_proto::BatchItem::Instr(instr) = item else {
                return Ok(());
            };
            let hex = to_hex(instr.hash());
            match instr {
                DeliveryInstr::Inline {
                    hash,
                    len_raw,
                    compression,
                    payload,
                } => {
                    inline_bytes += payload.len() as u64;
                    let raw = match compression {
                        cavs_proto::WIRE_COMPRESSION_NONE => payload,
                        cavs_proto::WIRE_COMPRESSION_ZSTD => {
                            zstd::bulk::decompress(&payload, len_raw as usize)
                                .map_err(|e| format!("descomprimiendo chunk {hex}: {e}"))?
                        }
                        other => return Err(format!("unknown wire compression {other}")),
                    };
                    if raw.len() != len_raw as usize || cavs_hash::hash_chunk(&raw) != hash {
                        return Err(format!("inline chunk {hex} failed hash verification"));
                    }
                    cache.put(&hash, &raw).map_err(|e| e.to_string())?;
                    inline_raw_bytes += raw.len() as u64;
                    inline_count += 1;
                }
                DeliveryInstr::Ref { hash } => {
                    ref_count += 1;
                    // Bloom false positive: server thinks we have it, we don't.
                    if !cache.contains(&hex) {
                        missing_refs.push(hash);
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("bad batch payload: {e}"))?;
    }

    // 3b. Repair bloom false positives: fetch each missing referenced chunk
    //     directly by hash, verify and cache it.
    missing_refs.sort_unstable();
    missing_refs.dedup();
    if !missing_refs.is_empty() {
        eprintln!(
            "[fetch] repairing {} bloom false-positive ref(s)",
            missing_refs.len()
        );
        for hash in &missing_refs {
            let hex = to_hex(hash);
            let raw = http_get_bytes(agent, &format!("{server}/api/assets/{asset}/chunks/{hex}"))?;
            if cavs_hash::hash_chunk(&raw) != *hash {
                bail!("repaired chunk {hex} failed hash verification");
            }
            cache.put(hash, &raw)?;
            inline_bytes += raw.len() as u64;
            inline_raw_bytes += raw.len() as u64;
            inline_count += 1;
        }
    }

    // 4. Reconstrucción streaming a disco: temporal .part -> verificar
    //    sha256 -> rename. Peak RAM = one chunk, not the whole asset.
    let primaries = reconstruct_streaming(&manifest, &cache, output)?;

    let logical: u64 = manifest_logical_bytes(&manifest);
    println!(
        "fetched : {asset} -> {} ({} tracks, {} segments)",
        output.display(),
        manifest.tracks.len(),
        manifest.segments.len()
    );
    println!(
        "egress  : {} wire inline ({} raw, {} chunks) / {} refs resolved from cache",
        human_bytes(inline_bytes),
        human_bytes(inline_raw_bytes),
        inline_count,
        ref_count
    );
    println!(
        "logical : {}  -> saved {:.2}% of egress",
        human_bytes(logical),
        if logical == 0 {
            0.0
        } else {
            (logical.saturating_sub(inline_bytes)) as f64 * 100.0 / logical as f64
        }
    );
    Ok((
        primaries,
        FetchStats {
            inline_bytes,
            inline_raw_bytes,
            inline_chunks: inline_count,
            refs: ref_count,
            logical_bytes: logical,
            delivery_mode: if inline_count == 0 {
                "references"
            } else {
                "chunks"
            },
            seeded_chunks: 0,
            seed_ms: 0,
        },
    ))
}

/// The v2 bootstrap route: download the whole compressed artifact, verify it
/// end to end, install it atomically, and seed the local chunk cache by
/// slicing the installed file along the manifest's chunk plan. Constant
/// memory: the artifact streams to disk and chunks are read back one at a
/// time.
fn fetch_bootstrap(
    agent: &ureq::Agent,
    server: &str,
    asset: &str,
    manifest: &Manifest,
    session: &SessionOpenResponse,
    cache: &ChunkCache,
    output: &Path,
) -> Result<(Vec<PathBuf>, FetchStats)> {
    // The bootstrap covers exactly one raw data track (the packer only emits
    // it for single-input packs). Anything else falls back to chunks.
    let boot_name = manifest
        .meta
        .iter()
        .find(|(k, _)| k == "bootstrap.name")
        .map(|(_, v)| v.as_str())
        .context("manifest has no bootstrap.name meta")?;
    if manifest.tracks.len() != 1 {
        bail!("bootstrap requires a single-track asset");
    }
    let track = &manifest.tracks[0];
    if track.name != boot_name {
        bail!("bootstrap.name does not match the asset's track");
    }
    if track.name.contains("..") || track.name.starts_with('/') {
        bail!("unsafe track name: {}", track.name);
    }

    // 1. Stream the artifact to disk, hashing the wire bytes as they arrive.
    std::fs::create_dir_all(output)?;
    let zst_path = output.join(format!("{boot_name}.bootstrap.zst.part"));
    let resp = agent
        .get(&format!("{server}/api/assets/{asset}/bootstrap"))
        .call()
        .with_context(|| format!("GET {server}/api/assets/{asset}/bootstrap"))?;
    let mut reader = resp.into_reader();
    let mut file = std::io::BufWriter::new(std::fs::File::create(&zst_path)?);
    let mut hasher = cavs_hash::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    let mut wire_bytes = 0u64;
    loop {
        use std::io::{Read as _, Write as _};
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        wire_bytes += n as u64;
    }
    {
        use std::io::Write as _;
        file.flush()?;
    }
    drop(file);

    // 2. Verify the wire artifact against the server-announced BLAKE3.
    if let Some(expected) = &session.bootstrap_blake3 {
        let got = to_hex(&hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(&zst_path);
            bail!("bootstrap artifact failed BLAKE3 verification");
        }
    }
    eprintln!(
        "[fetch] bootstrap artifact: {} wire (chunk path estimate: {})",
        human_bytes(wire_bytes),
        session
            .estimated_chunk_payload
            .map(human_bytes)
            .unwrap_or_else(|| "?".into()),
    );

    // 3. Decompress streaming into the final artifact, verifying the
    //    packer's SHA-256 end to end; atomic rename via PartFile.
    let expected_sha = manifest
        .meta
        .iter()
        .find(|(k, _)| k.strip_prefix("sha256:") == Some(boot_name))
        .map(|(_, v)| v.as_str());
    let final_path = output.join(&track.name);
    let mut part = PartFile::create(final_path.clone(), expected_sha.is_some())?;
    let mut raw_bytes = 0u64;
    {
        use std::io::Read as _;
        let zst_file = std::fs::File::open(&zst_path)?;
        let mut dec = zstd::stream::read::Decoder::new(std::io::BufReader::new(zst_file))?;
        loop {
            let n = dec.read(&mut buf)?;
            if n == 0 {
                break;
            }
            part.append_bytes(&buf[..n])?;
            raw_bytes += n as u64;
        }
    }
    let installed = part.finish(expected_sha)?;
    let _ = std::fs::remove_file(&zst_path);

    // 4. Seed the chunk cache from the installed artifact using the
    //    manifest's chunk plan: every future update starts warm.
    let seed_started = std::time::Instant::now();
    let mut seeded = 0u64;
    {
        use std::io::Read as _;
        let mut segs: Vec<_> = manifest
            .segments
            .iter()
            .filter(|s| s.track_id == track.track_id)
            .collect();
        segs.sort_by_key(|s| (s.pts_start, s.segment_id));
        let mut file = std::io::BufReader::new(std::fs::File::open(&installed)?);
        let mut chunk_buf = Vec::new();
        for seg in segs {
            for c in &seg.chunks {
                chunk_buf.resize(c.len as usize, 0);
                file.read_exact(&mut chunk_buf)
                    .with_context(|| format!("bootstrap shorter than chunk plan at {}", c.hash))?;
                let hash = cavs_hash::from_hex(&c.hash)
                    .with_context(|| format!("bad chunk hash {}", c.hash))?;
                if cavs_hash::hash_chunk(&chunk_buf) != hash {
                    bail!("seeded chunk {} failed hash verification", c.hash);
                }
                cache.put(&hash, &chunk_buf)?;
                seeded += 1;
            }
        }
    }
    let seed_ms = seed_started.elapsed().as_millis() as u64;

    let logical = manifest_logical_bytes(manifest);
    println!(
        "fetched : {asset} -> {} (bootstrap route)",
        output.display()
    );
    println!(
        "egress  : {} wire bootstrap ({} raw) / cache seeded with {seeded} chunks in {seed_ms} ms",
        human_bytes(wire_bytes),
        human_bytes(raw_bytes),
    );
    println!(
        "logical : {}  -> saved {:.2}% of egress",
        human_bytes(logical),
        if logical == 0 {
            0.0
        } else {
            (logical.saturating_sub(wire_bytes)) as f64 * 100.0 / logical as f64
        }
    );
    Ok((
        vec![installed],
        FetchStats {
            inline_bytes: wire_bytes,
            inline_raw_bytes: raw_bytes,
            inline_chunks: 0,
            refs: 0,
            logical_bytes: logical,
            delivery_mode: "bootstrap",
            seeded_chunks: seeded,
            seed_ms,
        },
    ))
}

/// All chunk hashes an asset can reference, deduplicated.
fn manifest_chunk_hashes(manifest: &Manifest) -> Vec<String> {
    let mut set = std::collections::HashSet::new();
    for t in &manifest.tracks {
        for c in &t.init_chunks {
            set.insert(c.hash.clone());
        }
    }
    for s in &manifest.segments {
        for c in &s.chunks {
            set.insert(c.hash.clone());
        }
    }
    set.into_iter().collect()
}

fn manifest_logical_bytes(manifest: &Manifest) -> u64 {
    let mut total = 0u64;
    for t in &manifest.tracks {
        total += t.init_chunks.iter().map(|c| c.len as u64).sum::<u64>();
    }
    for s in &manifest.segments {
        total += s.chunks.iter().map(|c| c.len as u64).sum::<u64>();
    }
    total
}

/// Streaming writer for one output artifact: temp `.part` file, chunks
/// appended in order straight from the disk cache (one chunk in RAM at a
/// time, BLAKE3-verified by the cache read), optional SHA-256 running
/// digest, then atomic rename into place.
struct PartFile {
    file: std::io::BufWriter<std::fs::File>,
    part_path: PathBuf,
    final_path: PathBuf,
    hasher: Option<sha2::Sha256>,
}

impl PartFile {
    fn create(final_path: PathBuf, with_sha256: bool) -> Result<Self> {
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let part_path = final_path.with_extension(match final_path.extension() {
            Some(ext) => format!("{}.part", ext.to_string_lossy()),
            None => "part".to_string(),
        });
        let file = std::io::BufWriter::new(std::fs::File::create(&part_path)?);
        Ok(Self {
            file,
            part_path,
            final_path,
            hasher: with_sha256.then(|| {
                use sha2::Digest as _;
                sha2::Sha256::new()
            }),
        })
    }

    /// Append raw bytes (bootstrap decompression path), feeding the running
    /// SHA-256 when enabled.
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        use std::io::Write as _;
        if let Some(h) = &mut self.hasher {
            use sha2::Digest as _;
            h.update(bytes);
        }
        self.file.write_all(bytes)?;
        Ok(())
    }

    fn append_chunk(&mut self, cache: &ChunkCache, hash_hex: &str) -> Result<()> {
        use std::io::Write as _;
        let hash =
            cavs_hash::from_hex(hash_hex).with_context(|| format!("bad chunk hash {hash_hex}"))?;
        let bytes = cache
            .get(&hash)?
            .with_context(|| format!("chunk {hash_hex} missing after fetch"))?;
        if let Some(h) = &mut self.hasher {
            use sha2::Digest as _;
            h.update(&bytes);
        }
        self.file.write_all(&bytes)?;
        Ok(())
    }

    /// Flush, optionally verify the SHA-256 against `expected_hex`, and
    /// rename `.part` -> final. On mismatch the `.part` is removed and the
    /// final path is never touched.
    fn finish(mut self, expected_sha256: Option<&str>) -> Result<PathBuf> {
        use std::io::Write as _;
        self.file.flush()?;
        drop(self.file);
        if let (Some(h), Some(expected)) = (self.hasher.take(), expected_sha256) {
            use sha2::Digest as _;
            let digest: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if !digest.eq_ignore_ascii_case(expected) {
                let _ = std::fs::remove_file(&self.part_path);
                bail!(
                    "sha256 mismatch for {} (expected {expected}, got {digest})",
                    self.final_path.display()
                );
            }
        }
        std::fs::rename(&self.part_path, &self.final_path)?;
        Ok(self.final_path)
    }
}

/// Mirror of `cavs unpack`, streaming from the chunk cache: per video track
/// an HLS dir + combined mp4; data tracks at their logical names, verified
/// against the manifest's per-file SHA-256 when present.
fn reconstruct_streaming(
    manifest: &Manifest,
    cache: &ChunkCache,
    output: &Path,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output)?;
    let sha_by_name: std::collections::HashMap<&str, &str> = manifest
        .meta
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("sha256:").map(|n| (n, v.as_str())))
        .collect();
    let mut primaries = Vec::new();

    for track in &manifest.tracks {
        let mut segs: Vec<_> = manifest
            .segments
            .iter()
            .filter(|s| s.track_id == track.track_id)
            .collect();
        segs.sort_by_key(|s| (s.pts_start, s.segment_id));

        match track.kind.as_str() {
            "video" | "audio" => {
                let dir = output.join(&track.name);
                let mut init = PartFile::create(dir.join("init.mp4"), false)?;
                let mut combined =
                    PartFile::create(output.join(format!("{}.mp4", track.name)), false)?;
                for c in &track.init_chunks {
                    init.append_chunk(cache, &c.hash)?;
                    combined.append_chunk(cache, &c.hash)?;
                }
                init.finish(None)?;
                for (ordinal, seg) in segs.iter().enumerate() {
                    let mut part =
                        PartFile::create(dir.join(format!("seg_{ordinal:05}.m4s")), false)?;
                    for c in &seg.chunks {
                        part.append_chunk(cache, &c.hash)?;
                        combined.append_chunk(cache, &c.hash)?;
                    }
                    part.finish(None)?;
                }
                primaries.push(combined.finish(None)?);
            }
            _ => {
                if track.name.contains("..") || track.name.starts_with('/') {
                    bail!("unsafe track name: {}", track.name);
                }
                let expected = sha_by_name.get(track.name.as_str()).copied();
                let mut part = PartFile::create(output.join(&track.name), expected.is_some())?;
                for seg in &segs {
                    for c in &seg.chunks {
                        part.append_chunk(cache, &c.hash)?;
                    }
                }
                let path = part.finish(expected)?;
                if track.codec == "raw" {
                    primaries.push(path);
                }
            }
        }
    }
    Ok(primaries)
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

// ---------------------------------------------------------------------------
// Minimal HTTP helpers (plain HTTP origin for the MVP; TLS via rustls is the
// planned evolution).
// ---------------------------------------------------------------------------

fn http_get_string(agent: &ureq::Agent, url: &str) -> Result<String> {
    agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_string()
        .context("reading response body")
}

fn http_get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let mut out = Vec::new();
    resp.into_reader()
        .read_to_end(&mut out)
        .context("reading chunk body")?;
    Ok(out)
}

fn http_post_json(agent: &ureq::Agent, url: &str, body: &str) -> Result<String> {
    agent
        .post(url)
        .set("content-type", "application/json")
        .send_string(body)
        .with_context(|| format!("POST {url}"))?
        .into_string()
        .context("reading response body")
}

/// POST returning the body as a reader: large batches are consumed as a
/// stream (peak RAM = one chunk) instead of being buffered whole.
fn http_post_reader(
    agent: &ureq::Agent,
    url: &str,
    body: &str,
) -> Result<Box<dyn std::io::Read + Send + Sync + 'static>> {
    let resp = agent
        .post(url)
        .set("content-type", "application/json")
        .send_string(body)
        .with_context(|| format!("POST {url}"))?;
    Ok(resp.into_reader())
}
