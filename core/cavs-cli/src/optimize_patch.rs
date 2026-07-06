//! `cavs optimize-patch` / `cavs apply-patch` — experimental optimized
//! pairwise sidecars (v0.7.0).
//!
//! A `.cavspatch` wraps an external byte-level delta (bsdiff or xdelta3),
//! recompressed with zstd-19 or brotli-9, plus enough metadata to verify
//! both ends. Sidecars serve exactly one old→new pair: they are an
//! *optional* route next to CAVS chunk/hybrid delivery, generated only for
//! configured hot pairs (previous→latest is the sensible default). The
//! pair count grows O(N²) with versions — never generate all pairs.
//!
//! Wire layout: magic "CAVSPCH1", u16 version (LE), str algo,
//! str compression, var old_size, [32] old BLAKE3, var new_size,
//! [32] new BLAKE3, var payload_len, payload, [32] BLAKE3 trailer.

use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_hash::{hash_chunk, ChunkHash};
use cavs_proto::errors::ErrorCode;
use std::io::Read;
use std::path::Path;
use std::process::Command;

pub const PATCH_MAGIC: [u8; 8] = *b"CAVSPCH1";
pub const PATCH_VERSION: u16 = 1;

pub struct PatchHeader {
    pub algo: String,
    pub compression: String,
    pub old_size: u64,
    pub old_blake3: ChunkHash,
    pub new_size: u64,
    pub new_blake3: ChunkHash,
}

fn write_var(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_var(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for i in 0..10 {
        let Some(&byte) = input.get(i) else {
            bail!("truncated varint");
        };
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            *input = &input[i + 1..];
            return Ok(value);
        }
        shift += 7;
    }
    bail!("overlong varint")
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    write_var(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

fn read_str(input: &mut &[u8]) -> Result<String> {
    let len = read_var(input)? as usize;
    if len > input.len() || len > 256 {
        bail!("truncated string");
    }
    let (head, tail) = input.split_at(len);
    *input = tail;
    String::from_utf8(head.to_vec()).context("string not UTF-8")
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if n > input.len() {
        bail!("truncated");
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

pub fn generate(old: &Path, new: &Path, algo: &str, compression: &str, out: &Path) -> Result<()> {
    if !matches!(algo, "bsdiff" | "xdelta3") {
        bail!("--algo must be bsdiff or xdelta3");
    }
    let started = std::time::Instant::now();
    let old_bytes = std::fs::read(old).with_context(|| format!("reading {}", old.display()))?;
    let new_bytes = std::fs::read(new).with_context(|| format!("reading {}", new.display()))?;

    let raw_patch = run_diff_tool(algo, old, new)?;
    let payload = compress(&raw_patch, compression)?;

    let mut buf = Vec::with_capacity(payload.len() + 128);
    buf.extend_from_slice(&PATCH_MAGIC);
    buf.extend_from_slice(&PATCH_VERSION.to_le_bytes());
    write_str(algo, &mut buf);
    write_str(compression, &mut buf);
    write_var(old_bytes.len() as u64, &mut buf);
    buf.extend_from_slice(&hash_chunk(&old_bytes));
    write_var(new_bytes.len() as u64, &mut buf);
    buf.extend_from_slice(&hash_chunk(&new_bytes));
    write_var(payload.len() as u64, &mut buf);
    buf.extend_from_slice(&payload);
    let trailer = hash_chunk(&buf);
    buf.extend_from_slice(&trailer);
    std::fs::write(out, &buf).with_context(|| format!("cannot write {}", out.display()))?;

    println!(
        "sidecar : {} ({algo}+{compression}, {} for {} → {}, {} ms)",
        out.display(),
        human_bytes(buf.len() as u64),
        human_bytes(old_bytes.len() as u64),
        human_bytes(new_bytes.len() as u64),
        started.elapsed().as_millis(),
    );
    println!(
        "note    : sidecars serve exactly this old→new pair; generate them only \
         for hot pairs (pair count grows O(N²) with versions)"
    );
    Ok(())
}

pub fn apply(old: &Path, patch: &Path, out: &Path) -> Result<()> {
    let bytes = std::fs::read(patch).with_context(|| format!("cannot read {}", patch.display()))?;
    let (header, payload) = decode(&bytes)?;

    let old_bytes = std::fs::read(old)?;
    if old_bytes.len() as u64 != header.old_size || hash_chunk(&old_bytes) != header.old_blake3 {
        bail!(
            "{}",
            ErrorCode::ApplyHashMismatch.msg(format!(
                "{} is not the old version this patch expects",
                old.display()
            ))
        );
    }

    let raw_patch = decompress(payload, &header.compression)?;
    let dir = tempfile::tempdir()?;
    let patch_tmp = dir.path().join("patch.bin");
    std::fs::write(&patch_tmp, &raw_patch)?;
    let part = out.with_extension("cavspatch.part");
    run_apply_tool(&header.algo, old, &patch_tmp, &part)?;

    let produced = std::fs::read(&part)?;
    if produced.len() as u64 != header.new_size || hash_chunk(&produced) != header.new_blake3 {
        let _ = std::fs::remove_file(&part);
        bail!(
            "{}",
            ErrorCode::ApplyHashMismatch.msg("sidecar apply produced wrong output")
        );
    }
    std::fs::rename(&part, out)?;
    println!(
        "apply   : OK — {} ({} via {}+{})",
        out.display(),
        human_bytes(header.new_size),
        header.algo,
        header.compression,
    );
    Ok(())
}

pub fn decode(bytes: &[u8]) -> Result<(PatchHeader, &[u8])> {
    if bytes.len() < 8 + 2 + 32 || bytes[..8] != PATCH_MAGIC {
        bail!("{}", ErrorCode::PlanCorrupt.msg("not a .cavspatch"));
    }
    let body_len = bytes.len() - 32;
    let expected: ChunkHash = bytes[body_len..].try_into().unwrap();
    if hash_chunk(&bytes[..body_len]) != expected {
        bail!(
            "{}",
            ErrorCode::PlanCorrupt.msg(".cavspatch integrity trailer mismatch")
        );
    }
    let mut input = &bytes[8..body_len];
    let version = u16::from_le_bytes(take(&mut input, 2)?.try_into().unwrap());
    if version != PATCH_VERSION {
        bail!("unsupported .cavspatch version {version}");
    }
    let algo = read_str(&mut input)?;
    let compression = read_str(&mut input)?;
    let old_size = read_var(&mut input)?;
    let old_blake3: ChunkHash = take(&mut input, 32)?.try_into().unwrap();
    let new_size = read_var(&mut input)?;
    let new_blake3: ChunkHash = take(&mut input, 32)?.try_into().unwrap();
    let payload_len = read_var(&mut input)?;
    if payload_len as usize != input.len() {
        bail!(
            "{}",
            ErrorCode::PlanCorrupt.msg(".cavspatch payload length mismatch")
        );
    }
    Ok((
        PatchHeader {
            algo,
            compression,
            old_size,
            old_blake3,
            new_size,
            new_blake3,
        },
        input,
    ))
}

// ---------------------------------------------------------------------------
// External tools
// ---------------------------------------------------------------------------

fn missing(tool: &str) -> anyhow::Error {
    anyhow::anyhow!(ErrorCode::PairwiseToolMissing.msg(format!("{tool} not found on PATH")))
}

fn run_diff_tool(algo: &str, old: &Path, new: &Path) -> Result<Vec<u8>> {
    let dir = tempfile::tempdir()?;
    let patch = dir.path().join("raw.patch");
    let status = match algo {
        // bsdiff's output embeds bzip2; we still recompress the envelope
        // (zstd/brotli usually shave a little more).
        "bsdiff" => Command::new("bsdiff")
            .args([old.as_os_str(), new.as_os_str(), patch.as_os_str()])
            .status(),
        // -S djw: cheap secondary compression off; ours is applied on top.
        "xdelta3" => Command::new("xdelta3")
            .args(["-e", "-9", "-f", "-S", "djw"])
            .args(["-s"])
            .args([old.as_os_str(), new.as_os_str(), patch.as_os_str()])
            .status(),
        _ => unreachable!(),
    }
    .map_err(|_| missing(algo))?;
    if !status.success() {
        bail!("{algo} diff failed with {status}");
    }
    Ok(std::fs::read(&patch)?)
}

fn run_apply_tool(algo: &str, old: &Path, patch: &Path, out: &Path) -> Result<()> {
    let status = match algo {
        "bsdiff" => Command::new("bspatch")
            .args([old.as_os_str(), out.as_os_str(), patch.as_os_str()])
            .status()
            .map_err(|_| missing("bspatch"))?,
        "xdelta3" => Command::new("xdelta3")
            .args(["-d", "-f", "-s"])
            .args([old.as_os_str(), patch.as_os_str(), out.as_os_str()])
            .status()
            .map_err(|_| missing("xdelta3"))?,
        other => bail!("unknown patch algo {other}"),
    };
    if !status.success() {
        bail!("{algo} apply failed with {status}");
    }
    Ok(())
}

pub fn compress(data: &[u8], compression: &str) -> Result<Vec<u8>> {
    match compression {
        "none" => Ok(data.to_vec()),
        _ if compression.starts_with("zstd-") => {
            let level: i32 = compression[5..].parse().context("bad zstd level")?;
            Ok(zstd::bulk::compress(data, level)?)
        }
        _ if compression.starts_with("brotli-") => {
            let quality = &compression[7..];
            run_pipe("brotli", &["-c", &format!("--quality={quality}")], data)
        }
        other => bail!("unknown compression {other} (use zstd-N, brotli-N or none)"),
    }
}

fn decompress(data: &[u8], compression: &str) -> Result<Vec<u8>> {
    match compression {
        "none" => Ok(data.to_vec()),
        _ if compression.starts_with("zstd-") => {
            let mut out = Vec::new();
            zstd::stream::copy_decode(data, &mut out)?;
            Ok(out)
        }
        _ if compression.starts_with("brotli-") => run_pipe("brotli", &["-d", "-c"], data),
        other => bail!("unknown compression {other}"),
    }
}

/// Pipe `data` through an external filter; stdin is fed from a thread so
/// large payloads cannot deadlock on full pipes.
fn run_pipe(bin: &str, args: &[&str], data: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| missing(bin))?;
    let mut stdin = child.stdin.take().unwrap();
    let input = data.to_vec();
    let writer = std::thread::spawn(move || {
        use std::io::Write as _;
        let _ = stdin.write_all(&input);
    });
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out)?;
    let _ = writer.join();
    if !child.wait()?.success() {
        bail!("{bin} failed");
    }
    Ok(out)
}
