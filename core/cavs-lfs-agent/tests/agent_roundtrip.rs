//! Protocol-level integration tests: spawn the real agent binary and speak
//! the git-lfs custom transfer NDJSON dialogue against a directory remote.
//! No git-lfs installation required.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// One agent process with piped protocol streams.
struct Agent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Agent {
    fn spawn(remote: &Path, cache: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cavs-lfs-agent"))
            .arg("--remote")
            .arg(remote)
            .arg("--cache-dir")
            .arg(cache)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn agent");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").expect("agent stdin open");
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("read agent stdout");
        assert!(n > 0, "agent closed stdout unexpectedly");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad JSON from agent: {e}: {line}"))
    }

    /// Skip progress events (asserting bytesSoFar monotonicity) until the
    /// `complete` for `oid` arrives.
    fn recv_complete(&mut self, oid: &str) -> Value {
        let mut last_so_far = 0u64;
        loop {
            let msg = self.recv();
            match msg["event"].as_str() {
                Some("progress") => {
                    assert_eq!(msg["oid"], oid);
                    let so_far = msg["bytesSoFar"].as_u64().unwrap();
                    assert!(
                        so_far >= last_so_far,
                        "progress went backwards: {so_far} < {last_so_far}"
                    );
                    last_so_far = so_far;
                }
                Some("complete") => {
                    assert_eq!(msg["oid"], oid);
                    return msg;
                }
                other => panic!("unexpected event {other:?}: {msg}"),
            }
        }
    }

    fn init(&mut self, operation: &str) {
        self.send(json!({
            "event": "init", "operation": operation, "remote": "unused",
            "concurrent": false, "concurrenttransfers": 1
        }));
        let reply = self.recv();
        assert_eq!(reply, json!({}), "init should succeed");
    }

    fn terminate(mut self) {
        self.send(json!({"event": "terminate"}));
        drop(self.stdin);
        let status = self.child.wait().expect("agent exit");
        assert!(status.success(), "agent exited with {status}");
    }
}

/// Deterministic pseudo-random bytes (xorshift), so chunk boundaries are
/// stable across runs.
fn test_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x = seed | 1;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn oid_of(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

fn upload(remote: &Path, cache: &Path, work: &Path, data: &[u8]) -> String {
    let oid = oid_of(data);
    let src = work.join(&oid);
    std::fs::write(&src, data).unwrap();

    let mut agent = Agent::spawn(remote, cache, &[]);
    agent.init("upload");
    agent.send(json!({
        "event": "upload", "oid": oid, "size": data.len(), "path": src
    }));
    let done = agent.recv_complete(&oid);
    assert!(
        done.get("error").is_none(),
        "upload failed: {}",
        done["error"]
    );
    agent.terminate();
    oid
}

fn download(remote: &Path, cache: &Path, oid: &str) -> Vec<u8> {
    let mut agent = Agent::spawn(remote, cache, &[]);
    agent.init("download");
    agent.send(json!({"event": "download", "oid": oid, "size": 0}));
    let done = agent.recv_complete(oid);
    assert!(
        done.get("error").is_none(),
        "download failed: {}",
        done["error"]
    );
    let path = done["path"].as_str().expect("complete carries a path");
    let data = std::fs::read(path).expect("reconstructed file readable");
    agent.terminate();
    data
}

#[test]
fn round_trip_and_delta_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let cache_a = tmp.path().join("cache-a");
    let cache_b = tmp.path().join("cache-b");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    // v1: 8 MiB pushed, then pulled back byte-identical from a cold cache.
    let v1 = test_bytes(8 * 1024 * 1024, 42);
    let oid1 = upload(&remote, &cache_a, &work, &v1);

    assert!(remote
        .join("assets")
        .join(&oid1)
        .join("manifest.json")
        .is_file());
    assert!(remote
        .join("assets")
        .join(&oid1)
        .join("chunk-map.json")
        .is_file());
    assert!(remote.join("chunks").join("packs").is_dir());

    let got = download(&remote, &cache_b, &oid1);
    assert_eq!(got.len(), v1.len());
    assert_eq!(oid_of(&got), oid1, "reconstructed bytes differ");

    // v2: mutate 64 KiB in the middle + append 256 KiB. Only the changed
    // chunks should hit the store — pack growth stays far below file size.
    let mut v2 = v1.clone();
    let mid = v2.len() / 2;
    v2[mid..mid + 64 * 1024].copy_from_slice(&test_bytes(64 * 1024, 1337));
    v2.extend_from_slice(&test_bytes(256 * 1024, 7));

    let packs = remote.join("chunks").join("packs");
    let before = dir_size(&packs);
    let oid2 = upload(&remote, &cache_a, &work, &v2);
    let growth = dir_size(&packs).saturating_sub(before);
    assert!(
        growth < v2.len() as u64 / 4,
        "expected chunk-level dedup: packs grew {growth} bytes for a {} byte file",
        v2.len()
    );

    // Pull v2 with the warm cache that already holds v1's chunks.
    let got2 = download(&remote, &cache_b, &oid2);
    assert_eq!(oid_of(&got2), oid2);

    // Idempotent re-push of v1 must succeed quickly and change nothing.
    let before = dir_size(&packs);
    upload(&remote, &cache_a, &work, &v1);
    assert_eq!(dir_size(&packs), before, "re-push must not grow the store");
}

#[test]
fn unknown_oid_is_a_clean_404_and_agent_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let cache = tmp.path().join("cache");

    let mut agent = Agent::spawn(&remote, &cache, &[]);
    agent.init("download");
    let missing = "0".repeat(64);
    agent.send(json!({"event": "download", "oid": missing, "size": 1}));
    let done = agent.recv_complete(&missing);
    assert_eq!(done["error"]["code"], 404, "expected 404: {done}");

    // The agent must keep serving after a per-object failure.
    agent.send(json!({"event": "download", "oid": missing, "size": 1}));
    let done = agent.recv_complete(&missing);
    assert_eq!(done["error"]["code"], 404);
    agent.terminate();
}

/// Send an upload and kill the agent as soon as the first progress event
/// arrives (mid pack/ingest/export). Returns the oid.
fn upload_and_kill(remote: &Path, cache: &Path, work: &Path, data: &[u8]) -> String {
    let oid = oid_of(data);
    let src = work.join(&oid);
    std::fs::write(&src, data).unwrap();

    let mut agent = Agent::spawn(remote, cache, &[]);
    agent.init("upload");
    agent.send(json!({
        "event": "upload", "oid": oid, "size": data.len(), "path": src
    }));
    // Wait for evidence the transfer is underway, then kill mid-flight.
    let msg = agent.recv();
    assert_eq!(msg["event"], "progress", "expected progress, got {msg}");
    agent.child.kill().expect("kill agent");
    let _ = agent.child.wait();
    oid
}

#[test]
fn interrupted_upload_recovers_on_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let cache = tmp.path().join("cache");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let data = test_bytes(32 * 1024 * 1024, 2024);

    // Kill an upload mid-flight, twice — the store must never be corrupted
    // (stray .part packs are cleaned on the next open; the flock dies with
    // the process).
    let oid = upload_and_kill(&remote, &cache, &work, &data);
    upload_and_kill(&remote, &cache, &work, &data);

    // Retry: the re-push must fully repair (publish + export) …
    let oid2 = upload(&remote, &cache, &work, &data);
    assert_eq!(oid, oid2);
    assert!(remote
        .join("assets")
        .join(&oid)
        .join("manifest.json")
        .is_file());

    // … and the object must round-trip byte-identical.
    let got = download(&remote, &tmp.path().join("cache-dl"), &oid);
    assert_eq!(oid_of(&got), oid, "reconstructed bytes differ after crash");

    // A later unrelated upload into the same (previously crashed) store
    // must also work — the store survived the interruptions.
    let other = test_bytes(4 * 1024 * 1024, 555);
    upload(&remote, &cache, &work, &other);
}

#[test]
fn interrupted_download_recovers_on_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let cache_up = tmp.path().join("cache-up");
    let cache_dl = tmp.path().join("cache-dl");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let data = test_bytes(32 * 1024 * 1024, 31337);
    let oid = upload(&remote, &cache_up, &work, &data);

    // Start a download and kill the agent at the first progress event —
    // the chunk cache is left partial but every cached chunk is complete
    // and hash-verified (atomic tmp+rename writes).
    let mut agent = Agent::spawn(&remote, &cache_dl, &[]);
    agent.init("download");
    agent.send(json!({"event": "download", "oid": oid, "size": data.len()}));
    let msg = agent.recv();
    assert_eq!(msg["event"], "progress", "expected progress, got {msg}");
    agent.child.kill().expect("kill agent");
    let _ = agent.child.wait();

    // Retry with the same cache: partial chunks are reused, the rest is
    // fetched, and the result is byte-identical.
    let got = download(&remote, &cache_dl, &oid);
    assert_eq!(oid_of(&got), oid, "reconstructed bytes differ after crash");
}

#[test]
fn batch_uploads_share_one_session() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let cache = tmp.path().join("cache");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    // Several objects through ONE agent process (one push session), like a
    // real multi-object git push.
    let blobs: Vec<Vec<u8>> = (0..5)
        .map(|i| test_bytes(2 * 1024 * 1024, 9000 + i))
        .collect();
    let mut agent = Agent::spawn(&remote, &cache, &[]);
    agent.init("upload");
    let mut oids = Vec::new();
    for data in &blobs {
        let oid = oid_of(data);
        let src = work.join(&oid);
        std::fs::write(&src, data).unwrap();
        agent.send(json!({
            "event": "upload", "oid": oid, "size": data.len(), "path": src
        }));
        let done = agent.recv_complete(&oid);
        assert!(done.get("error").is_none(), "upload failed: {done}");
        oids.push(oid);
    }
    agent.terminate();

    // Session finalize aggregates the whole batch into ONE pack (Xet-style
    // xorb aggregation) instead of one pack per object. Packs are sharded
    // into `packs/<ab>/<hex>.cavspack`.
    let mut packs = 0;
    for shard in std::fs::read_dir(remote.join(".store").join("packs"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
    {
        packs += std::fs::read_dir(shard.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "cavspack"))
            .count();
    }
    assert_eq!(packs, 1, "5-object push must produce one aggregated pack");

    // Every object of the batch is independently fetchable.
    for (i, oid) in oids.iter().enumerate() {
        let got = download(&remote, &tmp.path().join("cache-dl"), oid);
        assert_eq!(&got, &blobs[i], "object {i} corrupted");
    }
}

/// A push interrupted after per-object acks but before terminate publishes
/// nothing (atomic session), and a full retry then publishes everything.
#[test]
fn killed_before_terminate_publishes_nothing_and_retry_repairs() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let cache = tmp.path().join("cache");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let blobs: Vec<Vec<u8>> = (0..3)
        .map(|i| test_bytes(2 * 1024 * 1024, 500 + i))
        .collect();

    let mut agent = Agent::spawn(&remote, &cache, &[]);
    agent.init("upload");
    let mut oids = Vec::new();
    for data in &blobs {
        let oid = oid_of(data);
        let src = work.join(&oid);
        std::fs::write(&src, data).unwrap();
        agent.send(json!({
            "event": "upload", "oid": oid, "size": data.len(), "path": src
        }));
        let done = agent.recv_complete(&oid);
        assert!(done.get("error").is_none(), "upload failed: {done}");
        oids.push(oid);
    }
    // Kill without terminate: the session never finalized.
    agent.child.kill().expect("kill agent");
    let _ = agent.child.wait();

    for oid in &oids {
        assert!(
            !remote
                .join("assets")
                .join(oid)
                .join("manifest.json")
                .is_file(),
            "nothing may be published before finalize"
        );
    }

    // Retry the whole push (git-lfs re-sends every object): full repair.
    for data in &blobs {
        upload(&remote, &cache, &work, data);
    }
    for (i, oid) in oids.iter().enumerate() {
        let got = download(&remote, &tmp.path().join("cache-dl"), oid);
        assert_eq!(&got, &blobs[i], "object {i} corrupted after crash+retry");
    }
}

#[test]
fn upload_to_http_remote_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    let src = tmp.path().join("blob");
    std::fs::write(&src, b"data").unwrap();

    let mut agent = Agent::spawn(Path::new("https://cdn.example.invalid/lfs"), &cache, &[]);
    agent.init("upload");
    let oid = oid_of(b"data");
    agent.send(json!({"event": "upload", "oid": oid, "size": 4, "path": src}));
    let done = agent.recv_complete(&oid);
    let msg = done["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("read-only"),
        "expected read-only rejection, got: {done}"
    );
    agent.terminate();
}
