//! v0.3.0 compact manifest, end to end: format negotiation on the real
//! server, client fetch over binary v2, JSON v1 compatibility, and the
//! manifest metrics in stats-json.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
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

fn spawn_server(cavs_files: &[&Path]) -> (ServerGuard, String) {
    let mut cmd = Command::new(bin("cavs-server"));
    for f in cavs_files {
        cmd.arg(f);
    }
    let mut child = cmd
        .args(["--listen", "127.0.0.1:0"])
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

fn get_manifest(url: &str, accept: Option<&str>) -> Vec<u8> {
    let mut req = ureq::get(url);
    if let Some(accept) = accept {
        req = req.set("accept", accept);
    }
    let resp = req.call().expect("manifest request failed");
    let mut out = Vec::new();
    resp.into_reader().read_to_end(&mut out).unwrap();
    out
}

fn pack_asset(dir: &Path) -> PathBuf {
    let payload = pseudo_random(4 * 1024 * 1024, 77);
    let pck = dir.join("game_v1.pck");
    std::fs::write(&pck, &payload).unwrap();
    let cavs = dir.join("game_v1.cavs");
    let (ok, out) = run(
        "cavs",
        &[
            "pack",
            "--raw",
            pck.to_str().unwrap(),
            "-o",
            cavs.to_str().unwrap(),
        ],
    );
    assert!(ok, "pack failed:\n{out}");
    cavs
}

#[test]
fn server_negotiates_manifest_formats() {
    let dir = tempfile::tempdir().unwrap();
    let cavs = pack_asset(dir.path());
    let (_guard, url) = spawn_server(&[&cavs]);
    let endpoint = format!("{url}/api/assets/game_v1/manifest");

    // Default (no Accept): JSON v1, so v0.2.x clients keep working.
    let json_bytes = get_manifest(&endpoint, None);
    assert_eq!(json_bytes.first(), Some(&b'{'), "default must stay JSON");

    // Accept header: compact binary v2.
    let binary_bytes = get_manifest(&endpoint, Some(cavs_manifest::MANIFEST_V2_CONTENT_TYPE));
    assert!(
        binary_bytes.starts_with(cavs_manifest::MANIFEST_V2_MAGIC),
        "Accept negotiation must produce binary v2"
    );

    // Explicit query params override.
    let forced_binary = get_manifest(&format!("{endpoint}?format=binary-v2"), None);
    assert!(forced_binary.starts_with(cavs_manifest::MANIFEST_V2_MAGIC));
    let forced_json = get_manifest(
        &format!("{endpoint}?format=json-v1"),
        Some(cavs_manifest::MANIFEST_V2_CONTENT_TYPE),
    );
    assert_eq!(forced_json.first(), Some(&b'{'));

    // Both formats decode to the exact same runtime manifest.
    let from_json = cavs_manifest::read_manifest(&json_bytes).unwrap();
    let from_binary = cavs_manifest::read_manifest(&binary_bytes).unwrap();
    assert_eq!(from_json.format, cavs_manifest::ManifestFormat::JsonV1);
    assert_eq!(from_binary.format, cavs_manifest::ManifestFormat::BinaryV2);
    assert_eq!(
        serde_json::to_value(&from_json.manifest).unwrap(),
        serde_json::to_value(&from_binary.manifest).unwrap(),
        "v1 and v2 must carry the same manifest"
    );

    // The compact format must actually be compact (acceptance: >=50%).
    assert!(
        binary_bytes.len() * 2 <= json_bytes.len(),
        "binary v2 ({}) must be at least 50% smaller than JSON v1 ({})",
        binary_bytes.len(),
        json_bytes.len()
    );
}

#[test]
fn fetch_uses_binary_manifest_and_reconstructs_identically() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let cavs = pack_asset(d);
    let (_guard, url) = spawn_server(&[&cavs]);

    let out1 = d.join("install");
    let stats1 = d.join("cold.json");
    let (ok, log) = run(
        "cavs-client",
        &[
            "fetch",
            &url,
            "game_v1",
            "-o",
            out1.to_str().unwrap(),
            "--cache",
            d.join("cache").to_str().unwrap(),
            "--stats-json",
            stats1.to_str().unwrap(),
        ],
    );
    assert!(ok, "cold fetch failed:\n{log}");

    let stats: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&stats1).unwrap()).unwrap();
    let manifest = &stats["manifest"];
    assert_eq!(manifest["format"], "binary-v2");
    assert!(manifest["wire_bytes"].as_u64().unwrap() > 0);
    assert!(manifest["parse_ms"].as_f64().unwrap() >= 0.0);
    assert!(manifest["chunk_count_unique"].as_u64().unwrap() > 0);
    assert!(
        manifest["chunk_count_logical"].as_u64().unwrap()
            >= manifest["chunk_count_unique"].as_u64().unwrap()
    );

    // Byte-identical reconstruction through the binary manifest path.
    assert_eq!(
        std::fs::read(out1.join("game_v1.pck")).unwrap(),
        std::fs::read(d.join("game_v1.pck")).unwrap()
    );

    // Warm re-fetch: everything from cache, still through binary v2.
    let stats2 = d.join("warm.json");
    let (ok, log) = run(
        "cavs-client",
        &[
            "fetch",
            &url,
            "game_v1",
            "-o",
            d.join("install2").to_str().unwrap(),
            "--cache",
            d.join("cache").to_str().unwrap(),
            "--stats-json",
            stats2.to_str().unwrap(),
        ],
    );
    assert!(ok, "warm fetch failed:\n{log}");
    let stats: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&stats2).unwrap()).unwrap();
    assert_eq!(stats["inline_bytes"], 0, "warm re-fetch must cost 0 bytes");
    assert_eq!(stats["manifest"]["format"], "binary-v2");
}

/// A client that only speaks JSON v1 (a v0.2.x client) keeps working:
/// the default endpoint response parses as the same manifest.
#[test]
fn json_v1_stays_compatible_for_old_clients() {
    let dir = tempfile::tempdir().unwrap();
    let cavs = pack_asset(dir.path());
    let (_guard, url) = spawn_server(&[&cavs]);

    let body = get_manifest(&format!("{url}/api/assets/game_v1/manifest"), None);
    let manifest: cavs_proto::Manifest = serde_json::from_slice(&body).expect("v1 JSON broke");
    assert_eq!(manifest.asset, "game_v1");
    assert!(!manifest.chunk_table.is_empty());
}
