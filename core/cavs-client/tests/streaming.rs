//! End-to-end streaming test: real cavs-server + cavs-client over HTTP.
//!
//! Packs a raw asset, serves it, fetches twice with the same persistent
//! cache and asserts the second (warm) fetch downloads ~zero inline bytes.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

fn bin(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps/
    path.pop(); // debug/
    path.push(name);
    path
}

fn run(binary: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(bin(binary))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {binary}: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn cavs-server on an ephemeral port and return (guard, base_url).
fn spawn_server(cavs_file: &str) -> (ServerGuard, String) {
    let mut child = Command::new(bin("cavs-server"))
        .args([cavs_file, "--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn cavs-server");
    let stdout = child.stdout.take().unwrap();
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("server did not print its address");
    let url = line
        .trim()
        .strip_prefix("listening on ")
        .expect("unexpected server banner")
        .to_string();
    (ServerGuard(child), url)
}

fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut state = seed;
    for b in out.iter_mut() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (state >> 24) as u8;
    }
    out
}

fn parse_inline_bytes(output: &str) -> f64 {
    // "egress  : 1.91 MiB inline (9 chunks) / 0 refs resolved from cache"
    let line = output
        .lines()
        .find(|l| l.starts_with("egress"))
        .unwrap_or_else(|| panic!("no egress line in:\n{output}"));
    let rest = line.split(':').nth(1).unwrap().trim();
    let mut parts = rest.split_whitespace();
    let value: f64 = parts.next().unwrap().parse().unwrap();
    let unit = parts.next().unwrap();
    let mult = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        other => panic!("unexpected unit {other}"),
    };
    value * mult
}

#[test]
fn cold_then_warm_fetch_saves_egress() {
    let dir = tempfile::tempdir().unwrap();

    // Pack a raw asset with the CLI.
    let payload = pseudo_random(2_000_000, 77);
    let src = dir.path().join("bundle.bin");
    std::fs::write(&src, &payload).unwrap();
    let cavs = dir.path().join("bundle.cavs");
    let (ok, out) = run(
        "cavs",
        &[
            "pack",
            "--raw",
            src.to_str().unwrap(),
            "-o",
            cavs.to_str().unwrap(),
        ],
    );
    assert!(ok, "pack failed:\n{out}");

    let (_guard, url) = spawn_server(cavs.to_str().unwrap());

    // List.
    let (ok, out) = run("cavs-client", &["list", &url]);
    assert!(ok, "list failed:\n{out}");
    assert!(out.contains("bundle"), "asset missing from list:\n{out}");

    let cache = dir.path().join("cache");

    // Cold fetch: everything arrives inline.
    let out1 = dir.path().join("fetch1");
    let (ok, cold) = run(
        "cavs-client",
        &[
            "fetch",
            &url,
            "bundle",
            "-o",
            out1.to_str().unwrap(),
            "--cache",
            cache.to_str().unwrap(),
        ],
    );
    assert!(ok, "cold fetch failed:\n{cold}");
    let cold_bytes = parse_inline_bytes(&cold);
    assert!(
        cold_bytes > 1_900_000.0,
        "cold fetch should download ~all payload, got {cold_bytes} bytes:\n{cold}"
    );
    assert_eq!(std::fs::read(out1.join("bundle.bin")).unwrap(), payload);

    // Warm fetch, same cache: server plans refs, inline drops to zero.
    let out2 = dir.path().join("fetch2");
    let (ok, warm) = run(
        "cavs-client",
        &[
            "fetch",
            &url,
            "bundle",
            "-o",
            out2.to_str().unwrap(),
            "--cache",
            cache.to_str().unwrap(),
        ],
    );
    assert!(ok, "warm fetch failed:\n{warm}");
    let warm_bytes = parse_inline_bytes(&warm);
    assert_eq!(
        warm_bytes, 0.0,
        "warm fetch must be all refs:\n{warm}"
    );
    assert_eq!(std::fs::read(out2.join("bundle.bin")).unwrap(), payload);
}

/// TLS (self-signed + --ca) and Ed25519 content signature (--pubkey),
/// end to end over HTTPS.
#[test]
fn tls_and_signature_verification() {
    let dir = tempfile::tempdir().unwrap();

    // Keypair + signed pack.
    let key = dir.path().join("publisher.key");
    let (ok, out) = run("cavs", &["keygen", "-o", key.to_str().unwrap()]);
    assert!(ok, "keygen failed:\n{out}");
    let pubkey_file = key.with_extension("pub");
    let pubkey_hex = std::fs::read_to_string(&pubkey_file).unwrap().trim().to_string();

    let payload = pseudo_random(800_000, 33);
    let src = dir.path().join("asset.bin");
    std::fs::write(&src, &payload).unwrap();
    let cavs = dir.path().join("asset.cavs");
    let (ok, out) = run(
        "cavs",
        &[
            "pack", "--raw",
            src.to_str().unwrap(),
            "-o", cavs.to_str().unwrap(),
            "--sign-key", key.to_str().unwrap(),
        ],
    );
    assert!(ok, "signed pack failed:\n{out}");

    // `cavs verify --pubkey` accepts the right key and rejects a wrong one.
    let (ok, out) = run("cavs", &["verify", cavs.to_str().unwrap(), "--pubkey", &pubkey_hex]);
    assert!(ok && out.contains("signer matches"), "verify --pubkey failed:\n{out}");
    let wrong = "ab".repeat(32);
    let (ok, _) = run("cavs", &["verify", cavs.to_str().unwrap(), "--pubkey", &wrong]);
    assert!(!ok, "verify must fail with a wrong pubkey");

    // HTTPS server with self-signed cert.
    let tls_dir = dir.path().join("tls");
    let mut child = Command::new(bin("cavs-server"))
        .args([
            cavs.to_str().unwrap(),
            "--listen", "127.0.0.1:0",
            "--tls-self-signed", tls_dir.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn cavs-server (tls)");
    let stdout = child.stdout.take().unwrap();
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let _guard = ServerGuard(child);
    let url = line.trim().strip_prefix("listening on ").unwrap().to_string();
    assert!(url.starts_with("https://"), "expected https banner, got {url}");
    let ca = tls_dir.join("cert.pem");

    // Fetch over TLS with signature enforcement.
    let outdir = dir.path().join("restored");
    let (ok, out) = run(
        "cavs-client",
        &[
            "fetch", &url, "asset",
            "-o", outdir.to_str().unwrap(),
            "--cache", dir.path().join("cache").to_str().unwrap(),
            "--ca", ca.to_str().unwrap(),
            "--pubkey", &pubkey_hex,
        ],
    );
    assert!(ok, "tls+signed fetch failed:\n{out}");
    assert!(out.contains("content signature OK"), "missing signature check:\n{out}");
    assert_eq!(std::fs::read(outdir.join("asset.bin")).unwrap(), payload);

    // Wrong trusted key must be rejected.
    let (ok, out) = run(
        "cavs-client",
        &[
            "fetch", &url, "asset",
            "-o", dir.path().join("r2").to_str().unwrap(),
            "--cache", dir.path().join("cache").to_str().unwrap(),
            "--ca", ca.to_str().unwrap(),
            "--pubkey", &wrong,
        ],
    );
    assert!(!ok && out.contains("untrusted key"), "wrong key must fail:\n{out}");

    // Without --ca, TLS trust must fail against the self-signed cert.
    let (ok, _) = run(
        "cavs-client",
        &["list", &url],
    );
    assert!(!ok, "TLS must fail without trusting the self-signed cert");
}

#[test]
fn hls_endpoints_serve_playable_artifacts() {
    if Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();

    // Small real clip -> .cavs
    let clip = dir.path().join("clip.mp4");
    assert!(Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "testsrc2=duration=4:size=320x180:rate=30"])
        .args(["-c:v", "libx264", "-preset", "veryfast"])
        .arg(&clip)
        .status()
        .unwrap()
        .success());
    let cavs = dir.path().join("clip.cavs");
    let (ok, out) = run(
        "cavs",
        &[
            "pack",
            clip.to_str().unwrap(),
            "-o",
            cavs.to_str().unwrap(),
            "--segment-time",
            "2",
        ],
    );
    assert!(ok, "pack failed:\n{out}");

    let (_guard, url) = spawn_server(cavs.to_str().unwrap());

    // The reconstructed HLS playlist must decode end-to-end straight from
    // the server (ffmpeg follows init.mp4 + segments through HTTP).
    let playlist = format!("{url}/hls/clip/clip/media.m3u8");
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-xerror", "-i"])
        .arg(&playlist)
        .args(["-f", "null", "-"])
        .status()
        .unwrap()
        .success();
    assert!(ok, "HLS stream from cavs-server must decode: {playlist}");
}

/// Global content-addressable store: ingest two versions (dedup across them),
/// serve with --store, update over HTTP, verify byte-identity, then GC.
#[test]
fn global_store_dedup_serve_and_gc() {
    let dir = tempfile::tempdir().unwrap();
    // v1 and v2 share a large region; v2 appends a new block.
    let shared = pseudo_random(2_000_000, 55);
    let v1 = shared.clone();
    let mut v2 = shared.clone();
    v2.extend_from_slice(&pseudo_random(300_000, 66));
    let p1 = dir.path().join("v1.bin");
    let p2 = dir.path().join("v2.bin");
    std::fs::write(&p1, &v1).unwrap();
    std::fs::write(&p2, &v2).unwrap();

    // Pack each version.
    let c1 = dir.path().join("v1.cavs");
    let c2 = dir.path().join("v2.cavs");
    assert!(run("cavs", &["pack", "--raw", p1.to_str().unwrap(), "-o", c1.to_str().unwrap()]).0);
    assert!(run("cavs", &["pack", "--raw", p2.to_str().unwrap(), "-o", c2.to_str().unwrap()]).0);

    // Ingest both into a global store.
    let store = dir.path().join("store");
    let sd = store.to_str().unwrap();
    assert!(run("cavs", &["store", sd, "add", "v1", c1.to_str().unwrap()]).0);
    let (ok, out) = run("cavs", &["store", sd, "add", "v2", c2.to_str().unwrap()]);
    assert!(ok, "store add v2 failed:\n{out}");
    // v2 must dedup most chunks against v1 (shared 2 MB region).
    assert!(out.contains("deduplicated"), "no dedup reported:\n{out}");
    let (_, stat) = run("cavs", &["store", sd, "stat"]);
    assert!(stat.contains("storage saved"), "no savings line:\n{stat}");

    // Serve from the store and fetch both, warm cache across versions.
    let mut child = Command::new(bin("cavs-server"))
        .args(["--store", sd, "--listen", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.trim().strip_prefix("listening on ").unwrap().to_string();
    let _guard = ServerGuard(child);

    let cache = dir.path().join("cache");
    let out1 = dir.path().join("f1");
    let out2 = dir.path().join("f2");
    assert!(run("cavs-client", &["fetch", &url, "v1", "-o", out1.to_str().unwrap(),
        "--cache", cache.to_str().unwrap()]).0);
    let s2 = dir.path().join("s2.json");
    assert!(run("cavs-client", &["fetch", &url, "v2", "-o", out2.to_str().unwrap(),
        "--cache", cache.to_str().unwrap(), "--stats-json", s2.to_str().unwrap()]).0);

    // Byte-identical reconstruction from the store.
    assert_eq!(std::fs::read(out1.join("v1.bin")).unwrap(), v1);
    assert_eq!(std::fs::read(out2.join("v2.bin")).unwrap(), v2);

    // The v2 update, with v1 already cached, must be a small fraction.
    let stats: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&s2).unwrap()).unwrap();
    let inline = stats["inline_bytes"].as_u64().unwrap();
    assert!(inline < 600_000, "v2 update should be small, got {inline} bytes");
    assert!(stats["refs"].as_u64().unwrap() > 0, "v2 should resolve refs from cache");

    // Unpublish v1, GC: v1-unique chunks reclaimed, v2 still fully intact.
    assert!(run("cavs", &["store", sd, "rm", "v1"]).0);
    assert!(run("cavs", &["store", sd, "gc", "--grace", "0"]).0);
    let (ok, _) = run("cavs-client", &["fetch", &url, "v2",
        "-o", dir.path().join("f3").to_str().unwrap(),
        "--cache", dir.path().join("c2").to_str().unwrap()]);
    assert!(ok, "v2 must still serve after v1 GC");
    assert_eq!(std::fs::read(dir.path().join("f3").join("v2.bin")).unwrap(), v2);
}

/// Force the Bloom-filter have-set path: an asset with far more than the
/// client's bloom threshold (256) of chunks, fetched warm. Reconstruction
/// must stay byte-identical (covers bloom membership + false-positive repair).
#[test]
fn bloom_haveset_large_asset() {
    let dir = tempfile::tempdir().unwrap();
    // ~40 MB of pseudo-random data -> ~600 chunks at the 64 KiB default,
    // well above the client's 256-chunk bloom threshold.
    let data = pseudo_random(40_000_000, 123);
    let src = dir.path().join("big.bin");
    std::fs::write(&src, &data).unwrap();
    let cavs = dir.path().join("big.cavs");
    assert!(run("cavs", &["pack", "--raw", src.to_str().unwrap(), "-o", cavs.to_str().unwrap()]).0);

    let (_guard, url) = spawn_server(cavs.to_str().unwrap());
    let cache = dir.path().join("cache");
    // Cold seeds the cache with hundreds of chunks.
    assert!(run("cavs-client", &["fetch", &url, "big",
        "-o", dir.path().join("f1").to_str().unwrap(), "--cache", cache.to_str().unwrap()]).0);
    // Warm: have-set > 256 -> client sends a Bloom filter.
    let s = dir.path().join("warm.json");
    let out = dir.path().join("f2");
    let (ok, log) = run("cavs-client", &["fetch", &url, "big",
        "-o", out.to_str().unwrap(), "--cache", cache.to_str().unwrap(),
        "--stats-json", s.to_str().unwrap()]);
    assert!(ok, "warm bloom fetch failed:\n{log}");
    assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), data, "bloom path must reconstruct exactly");
    let stats: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&s).unwrap()).unwrap();
    // Warm fetch resolves everything from cache; inline stays near zero even
    // accounting for the odd bloom false-positive repair.
    assert!(stats["refs"].as_u64().unwrap() > 256, "expected a large ref set via bloom");
    assert!(stats["inline_bytes"].as_u64().unwrap() < 2_000_000, "warm bloom fetch downloaded too much");
}
