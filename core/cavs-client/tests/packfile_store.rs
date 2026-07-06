//! v0.4.0 packfile storage, end to end over real HTTP: a packfile-layout
//! global store serves cold installs and updates byte-identically, pack
//! range reads coalesce, the binary manifest carries location hints, and
//! loose-layout stores keep working unchanged.

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

fn spawn_store_server(store: &Path) -> (ServerGuard, String) {
    let mut child = Command::new(bin("cavs-server"))
        .args([
            "--store",
            store.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ])
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

/// Two versions of a build: v2 rewrites a slice in the middle of v1.
fn make_versions(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let v1 = pseudo_random(5 * 1024 * 1024, 11);
    let mut v2 = v1.clone();
    let at = 2 * 1024 * 1024;
    v2[at..at + 200 * 1024].copy_from_slice(&pseudo_random(200 * 1024, 999));
    std::fs::write(dir.join("game_v1.pck"), &v1).unwrap();
    std::fs::write(dir.join("game_v2.pck"), &v2).unwrap();
    (v1, v2)
}

fn pack_and_ingest(dir: &Path, storage: &str) -> PathBuf {
    for v in ["v1", "v2"] {
        let (ok, out) = run(
            "cavs",
            &[
                "pack",
                "--raw",
                dir.join(format!("game_{v}.pck")).to_str().unwrap(),
                "-o",
                dir.join(format!("game_{v}.cavs")).to_str().unwrap(),
            ],
        );
        assert!(ok, "pack {v} failed:\n{out}");
    }
    let store = dir.join(format!("store-{storage}"));
    for v in ["v1", "v2"] {
        let (ok, out) = run(
            "cavs",
            &[
                "store",
                store.to_str().unwrap(),
                "add",
                &format!("game_{v}"),
                dir.join(format!("game_{v}.cavs")).to_str().unwrap(),
                "--storage",
                storage,
            ],
        );
        assert!(ok, "store add {v} ({storage}) failed:\n{out}");
    }
    store
}

fn fetch(url: &str, asset: &str, out: &Path, cache: &Path, stats: &Path) -> String {
    let (ok, log) = run(
        "cavs-client",
        &[
            "fetch",
            url,
            asset,
            "-o",
            out.to_str().unwrap(),
            "--cache",
            cache.to_str().unwrap(),
            "--stats-json",
            stats.to_str().unwrap(),
        ],
    );
    assert!(ok, "fetch {asset} failed:\n{log}");
    std::fs::read_to_string(stats).unwrap()
}

fn http_get(url: &str, accept: Option<&str>) -> Vec<u8> {
    let mut req = ureq::get(url);
    if let Some(a) = accept {
        req = req.set("accept", a);
    }
    let mut out = Vec::new();
    req.call()
        .expect("request failed")
        .into_reader()
        .read_to_end(&mut out)
        .unwrap();
    out
}

#[test]
fn packfile_store_serves_cold_update_warm_with_coalescing() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (v1, v2) = make_versions(d);
    let store = pack_and_ingest(d, "packfiles");

    // Physical layout: packs on disk, no loose chunk objects.
    assert!(store.join("packs").is_dir());
    let loose: Vec<_> = walk_files(&store.join("chunks"));
    assert!(loose.is_empty(), "loose chunks written in packfile mode");

    let (_guard, url) = spawn_store_server(&store);

    // Cold install of v1 (store mode has no bootstrap: chunk path).
    let cache = d.join("cache");
    let stats = fetch(&url, "game_v1", &d.join("out1"), &cache, &d.join("s1.json"));
    assert!(stats.contains("\"delivery_mode\":\"chunks\""));
    assert_eq!(std::fs::read(d.join("out1/game_v1.pck")).unwrap(), v1);

    // Update to v2: only changed chunks travel.
    let stats: serde_json::Value = serde_json::from_str(&fetch(
        &url,
        "game_v2",
        &d.join("out2"),
        &cache,
        &d.join("s2.json"),
    ))
    .unwrap();
    assert_eq!(std::fs::read(d.join("out2/game_v2.pck")).unwrap(), v2);
    let update_wire = stats["inline_bytes"].as_u64().unwrap();
    assert!(
        update_wire < v2.len() as u64 / 4,
        "update should be a fraction of the build: {update_wire}"
    );

    // Warm re-fetch: zero payload.
    let stats: serde_json::Value = serde_json::from_str(&fetch(
        &url,
        "game_v2",
        &d.join("out3"),
        &cache,
        &d.join("s3.json"),
    ))
    .unwrap();
    assert_eq!(stats["inline_bytes"], 0);

    // Coalescing (acceptance 7): physical pack reads < chunks requested.
    let metrics = String::from_utf8(http_get(&format!("{url}/metrics"), None)).unwrap();
    let counter = |name: &str| -> u64 {
        metrics
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("missing metric {name}:\n{metrics}"))
    };
    let requested = counter("cavs_pack_chunks_requested_total");
    let ranges = counter("cavs_pack_ranges_read_total");
    assert!(requested > 0, "pack reads must be counted");
    assert!(
        ranges < requested,
        "coalescing must reduce physical reads: {ranges} ranges for {requested} chunks"
    );

    // The binary manifest of a packfile-store asset carries location hints.
    let manifest_bytes = http_get(
        &format!("{url}/api/assets/game_v2/manifest"),
        Some(cavs_manifest::MANIFEST_V2_CONTENT_TYPE),
    );
    let loaded = cavs_manifest::read_manifest(&manifest_bytes).unwrap();
    let locations = loaded.locations.expect("packfile asset must carry hints");
    assert_eq!(
        locations.len(),
        loaded.manifest.chunk_table.len(),
        "every chunk should have a location hint"
    );

    // ETag on the immutable chunk endpoint.
    let some_chunk = &loaded.manifest.chunk_table[0];
    let resp = ureq::get(&format!("{url}/api/assets/game_v2/chunks/{some_chunk}"))
        .call()
        .unwrap();
    assert_eq!(
        resp.header("etag"),
        Some(format!("\"blake3-{some_chunk}\"").as_str())
    );
}

#[test]
fn loose_store_still_works_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (v1, _) = make_versions(d);
    let store = pack_and_ingest(d, "loose");
    assert!(!store.join("packs").is_dir() || walk_files(&store.join("packs")).is_empty());

    let (_guard, url) = spawn_store_server(&store);
    let stats = fetch(
        &url,
        "game_v1",
        &d.join("out1"),
        &d.join("cache"),
        &d.join("s1.json"),
    );
    assert!(stats.contains("\"delivery_mode\":\"chunks\""));
    assert_eq!(std::fs::read(d.join("out1/game_v1.pck")).unwrap(), v1);

    // Loose assets carry no location hints.
    let manifest_bytes = http_get(
        &format!("{url}/api/assets/game_v1/manifest"),
        Some(cavs_manifest::MANIFEST_V2_CONTENT_TYPE),
    );
    assert!(cavs_manifest::read_manifest(&manifest_bytes)
        .unwrap()
        .locations
        .is_none());
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_files(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}
