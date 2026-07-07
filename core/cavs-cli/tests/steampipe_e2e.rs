//! End-to-end tests for the v0.9.0 SteamPipe-class analysis commands,
//! driving the real `cavs` binary: model estimates, diagnostics,
//! workspace/depot/branch flow, install plans, io-estimate, plan-update,
//! Godot PCK analysis and build signing/encryption.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cavs_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps/
    path.pop(); // debug/
    path.push("cavs");
    path
}

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(cavs_bin())
        .args(args)
        .output()
        .expect("failed to run cavs binary");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// stdout only — for `--json` commands whose stderr carries notes.
fn run_json(args: &[&str]) -> serde_json::Value {
    let out = Command::new(cavs_bin())
        .args(args)
        .output()
        .expect("failed to run cavs binary");
    assert!(
        out.status.success(),
        "cavs {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is not JSON")
}

fn prng(len: usize, seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut state = seed;
    for b in out.iter_mut() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (state >> 24) as u8;
    }
    out
}

fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

/// v1: pack of 20 × 1 MiB assets. localized: one asset edited in place.
/// shuffled: same assets, rotated order plus a 4 KiB header shift.
fn pack_scenarios(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let assets: Vec<Vec<u8>> = (0..20).map(|i| prng(1 << 20, 500 + i as u32)).collect();
    let v1 = dir.join("v1");
    let localized = dir.join("v2-localized");
    let shuffled = dir.join("v2-shuffled");

    write(&v1.join("world.pak"), &assets.concat());

    let mut edited = assets.clone();
    edited[7][1000..1200].copy_from_slice(&prng(200, 9999));
    write(&localized.join("world.pak"), &edited.concat());

    let mut rotated: Vec<u8> = vec![0x42; 4096];
    for a in assets.iter().skip(1).chain(assets.iter().take(1)) {
        rotated.extend_from_slice(a);
    }
    write(&shuffled.join("world.pak"), &rotated);

    (v1, localized, shuffled)
}

#[test]
fn steampipe_style_bench_localized_vs_shuffled() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, localized, shuffled) = pack_scenarios(dir.path());

    let local = run_json(&[
        "bench",
        "steampipe-style",
        v1.to_str().unwrap(),
        localized.to_str().unwrap(),
        "--json",
    ]);
    // One dirtied 1 MiB chunk out of 20.
    assert_eq!(local["new_or_changed_chunks"], 1);
    assert_eq!(local["files_modified"], 1);
    assert!(local["note"].as_str().unwrap().contains("not Valve"));

    let shuf = run_json(&[
        "bench",
        "steampipe-style",
        v1.to_str().unwrap(),
        shuffled.to_str().unwrap(),
        "--json",
    ]);
    // Every fixed window slides: the model loses (almost) everything.
    let changed = shuf["new_or_changed_chunks"].as_u64().unwrap();
    let total = shuf["total_chunks_new"].as_u64().unwrap();
    assert!(changed >= total - 1, "changed {changed} of {total}");
}

#[test]
fn analyze_steampipe_diagnoses_shuffling_with_recommendation() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, _, shuffled) = pack_scenarios(dir.path());

    let report = run_json(&[
        "analyze",
        "steampipe",
        v1.to_str().unwrap(),
        shuffled.to_str().unwrap(),
        "--json",
    ]);
    let kinds: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"asset_shuffling"), "kinds: {kinds:?}");
    // CAVS content-defined estimate must be far below the fixed model.
    let steam = report["estimated_steampipe_download"].as_u64().unwrap();
    let cavs = report["estimated_cavs_download"].as_u64().unwrap();
    assert!(cavs * 4 < steam, "cavs {cavs} vs steam {steam}");

    // Markdown report with the mandatory labeling.
    let md_path = dir.path().join("analysis.md");
    let (ok, _) = run(&[
        "analyze",
        "steampipe",
        v1.to_str().unwrap(),
        shuffled.to_str().unwrap(),
        "--out",
        md_path.to_str().unwrap(),
    ]);
    assert!(ok);
    let md = std::fs::read_to_string(&md_path).unwrap();
    assert!(md.contains("not Valve's exact SteamPipe implementation"));
    assert!(md.contains("Recommended fix"));
}

#[test]
fn analyze_packs_reports_scatteredness_table() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, localized, _) = pack_scenarios(dir.path());
    let report = run_json(&[
        "analyze-packs",
        v1.to_str().unwrap(),
        localized.to_str().unwrap(),
        "--json",
    ]);
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["file"], "world.pak");
    assert_eq!(rows[0]["changed_windows_1m"], 1);
    assert_eq!(rows[0]["main_issue"], "localized");
}

#[test]
fn io_estimate_shows_rebuild_cost_dominating() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, localized, _) = pack_scenarios(dir.path());
    let report = run_json(&[
        "io-estimate",
        v1.to_str().unwrap(),
        localized.to_str().unwrap(),
        "--json",
    ]);
    let routes = report["routes"].as_array().unwrap();
    let steam = routes
        .iter()
        .find(|r| r["route"].as_str().unwrap().contains("SteamPipe-style"))
        .unwrap();
    // A ~200-byte edit still rebuilds the whole 20 MiB pack locally.
    assert_eq!(steam["write_bytes"].as_u64().unwrap(), 20 << 20);
    assert!(steam["io_dominates_network"].as_bool().unwrap());
    assert!(steam["device_seconds"]["hdd"].as_f64().unwrap() > 0.0);
    // Full download reads nothing old.
    let full = routes
        .iter()
        .find(|r| r["route"].as_str().unwrap().contains("full"))
        .unwrap();
    assert_eq!(full["read_old_bytes"].as_u64().unwrap(), 0);
}

#[test]
fn plan_update_policies_and_states() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, localized, _) = pack_scenarios(dir.path());
    let report = run_json(&[
        "plan-update",
        "--from",
        v1.to_str().unwrap(),
        "--to",
        localized.to_str().unwrap(),
        "--client-state",
        "has-previous-install,low-ram",
        "--policy",
        "network_min",
        "--json",
    ]);
    let chosen = report["chosen"].as_str().unwrap();
    assert!(
        chosen.contains("cavsplan") || chosen.contains("hybrid"),
        "chosen {chosen}"
    );
    assert!(!report["reason"].as_str().unwrap().is_empty());
    // Unavailable routes exist and are never the choice.
    let routes = report["routes"].as_array().unwrap();
    assert!(routes.iter().any(|r| r["available"] == false));
    for r in routes {
        if r["available"] == false {
            assert_ne!(r["route"].as_str().unwrap(), chosen);
        }
    }
}

#[test]
fn publish_preview_recommends_verified_route() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, localized, _) = pack_scenarios(dir.path());
    let report = run_json(&[
        "publish-preview",
        localized.to_str().unwrap(),
        "--previous",
        v1.to_str().unwrap(),
        "--routes",
        "cavs", // skip butler/pairwise probing in CI
        "--json",
    ]);
    assert!(report["note"].as_str().unwrap().contains("not Valve"));
    assert!(report["steampipe_style"]["network_bytes"].as_u64().unwrap() > 0);
    let recommended = report["recommended"].as_str().unwrap();
    assert!(
        recommended.contains("cavsplan") || recommended.contains("chunk"),
        "recommended {recommended}"
    );
}

#[test]
fn workspace_depots_branches_builds_and_install_plans() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let ws_s = ws.to_str().unwrap();

    // Shared + platform + optional depots.
    let shared = prng(2 << 20, 1);
    write(&dir.path().join("v1/shared/data.bin"), &shared);
    write(&dir.path().join("v1/win/game.exe"), &prng(1 << 20, 2));
    write(&dir.path().join("v1/hd/tex.bin"), &prng(1 << 20, 3));
    // v2: shared gets a localized edit.
    let mut shared2 = shared.clone();
    shared2[100..200].copy_from_slice(&prng(100, 4));
    write(&dir.path().join("v2/shared/data.bin"), &shared2);
    write(&dir.path().join("v2/win/game.exe"), &prng(1 << 20, 2));
    write(&dir.path().join("v2/hd/tex.bin"), &prng(1 << 20, 3));

    let v = |p: &str| dir.path().join(p).display().to_string();
    assert!(run(&["workspace", "init", ws_s, "--app", "my-game"]).0);
    assert!(run(&["depot", "add", "base", "--workspace", ws_s]).0);
    assert!(
        run(&[
            "depot",
            "add",
            "windows",
            "--workspace",
            ws_s,
            "--platform",
            "windows"
        ])
        .0
    );
    assert!(
        run(&[
            "depot",
            "add",
            "hd-textures",
            "--workspace",
            ws_s,
            "--optional"
        ])
        .0
    );
    assert!(run(&["branch", "add", "public", "--workspace", ws_s]).0);
    assert!(run(&["branch", "add", "beta", "--workspace", ws_s]).0);

    let base1 = format!("base={}", v("v1/shared"));
    let win1 = format!("windows={}", v("v1/win"));
    let hd1 = format!("hd-textures={}", v("v1/hd"));
    assert!(
        run(&[
            "build",
            "create",
            "--workspace",
            ws_s,
            "--branch",
            "beta",
            "--depot",
            &base1,
            "--depot",
            &win1,
            "--depot",
            &hd1,
            "--label",
            "v1",
        ])
        .0
    );
    let base2 = format!("base={}", v("v2/shared"));
    let win2 = format!("windows={}", v("v2/win"));
    let hd2 = format!("hd-textures={}", v("v2/hd"));
    assert!(
        run(&[
            "build",
            "create",
            "--workspace",
            ws_s,
            "--branch",
            "beta",
            "--depot",
            &base2,
            "--depot",
            &win2,
            "--depot",
            &hd2,
            "--label",
            "v2",
        ])
        .0
    );

    // Promote/rollback rules.
    assert!(
        run(&[
            "branch",
            "promote",
            "--workspace",
            ws_s,
            "--branch",
            "public",
            "--build",
            "build_1002"
        ])
        .0
    );
    let (ok, out) = run(&[
        "branch",
        "rollback",
        "--workspace",
        ws_s,
        "--branch",
        "public",
        "--to",
        "build_1001",
    ]);
    assert!(!ok, "public never served build_1001");
    assert!(out.contains("CAVS-E-BUILD-NOT-FOUND"), "{out}");
    assert!(
        run(&[
            "branch",
            "rollback",
            "--workspace",
            ws_s,
            "--branch",
            "beta",
            "--to",
            "build_1001"
        ])
        .0
    );

    // Install plan: linux player without HD ownership gets base only.
    let plan = run_json(&[
        "install-plan",
        "--workspace",
        ws_s,
        "--branch",
        "public",
        "--platform",
        "linux",
        "--from",
        "build_1001",
        "--json",
    ]);
    let depots: Vec<&str> = plan["depots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["depot"].as_str().unwrap())
        .collect();
    assert_eq!(
        depots,
        vec!["base"],
        "windows filtered by platform, hd by ownership"
    );
    assert_eq!(plan["depots"][0]["action"], "update");
    // The localized edit costs far less than the whole depot.
    assert!(plan["total_bytes"].as_u64().unwrap() < 1 << 20);

    // Windows player owning hd-textures gets all three.
    let plan = run_json(&[
        "install-plan",
        "--workspace",
        ws_s,
        "--branch",
        "public",
        "--platform",
        "windows",
        "--owned",
        "base,windows,hd-textures",
        "--from",
        "build_1001",
        "--json",
    ]);
    assert_eq!(plan["depots"].as_array().unwrap().len(), 3);
    // Unchanged depots are no-ops.
    let windows_plan = plan["depots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["depot"] == "windows")
        .unwrap();
    assert_eq!(windows_plan["action"], "no-op");

    // Promote-preview estimates per depot.
    let (ok, out) = run(&[
        "branch",
        "promote-preview",
        "--workspace",
        ws_s,
        "--branch",
        "beta",
        "--build",
        "build_1002",
    ]);
    assert!(ok);
    assert!(out.contains("base"), "{out}");
}

/// Minimal Godot PCK v2 writer for the analyzer test.
fn synth_pck(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(&0x4350_4447u32.to_le_bytes()); // GDPC
    h.extend_from_slice(&2u32.to_le_bytes());
    for v in [4u32, 2, 0] {
        h.extend_from_slice(&v.to_le_bytes());
    }
    h.extend_from_slice(&0u32.to_le_bytes()); // flags
    h.extend_from_slice(&0u64.to_le_bytes()); // file_base
    h.extend_from_slice(&[0u8; 64]);
    h.extend_from_slice(&(files.len() as u32).to_le_bytes());
    let dir_size: usize = files
        .iter()
        .map(|(p, _)| 4 + p.len() + 8 + 8 + 16 + 4)
        .sum();
    let mut offset = (h.len() + dir_size) as u64;
    let mut payload = Vec::new();
    for (path, bytes) in files {
        h.extend_from_slice(&(path.len() as u32).to_le_bytes());
        h.extend_from_slice(path.as_bytes());
        h.extend_from_slice(&offset.to_le_bytes());
        h.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        h.extend_from_slice(&[0u8; 16]);
        h.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(bytes);
        offset += bytes.len() as u64;
    }
    h.extend(payload);
    h
}

#[test]
fn godot_pck_analyzer_maps_changes_to_resources() {
    let dir = tempfile::tempdir().unwrap();
    let hero = prng(256 * 1024, 11);
    let mut hero2 = hero.clone();
    hero2[1000..1100].copy_from_slice(&prng(100, 12));
    let level = prng(512 * 1024, 13);

    let old = synth_pck(&[
        ("res://textures/hero.png", &hero),
        ("res://levels/level01.scn", &level),
    ]);
    let new = synth_pck(&[
        ("res://textures/hero.png", &hero2),
        ("res://levels/level01.scn", &level),
    ]);
    let old_p = dir.path().join("old.pck");
    let new_p = dir.path().join("new.pck");
    std::fs::write(&old_p, &old).unwrap();
    std::fs::write(&new_p, &new).unwrap();

    let report = run_json(&[
        "analyze",
        "godot-pck",
        old_p.to_str().unwrap(),
        new_p.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(report["parsed"], true);
    assert_eq!(report["resources_total"], 2);
    let touched: Vec<&str> = report["changed_resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        touched.contains(&"res://textures/hero.png"),
        "touched: {touched:?}"
    );
    assert!(report["note"].as_str().unwrap().contains("not Valve"));
}

#[test]
fn build_sign_verify_encrypt_via_binary() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("release.key");
    let artifact = dir.path().join("build.cavs");
    std::fs::write(&artifact, prng(50_000, 77)).unwrap();

    assert!(run(&["keygen", "-o", key.to_str().unwrap()]).0);
    assert!(
        run(&[
            "build",
            "sign",
            artifact.to_str().unwrap(),
            "--key",
            key.to_str().unwrap()
        ])
        .0
    );
    let pubkey = dir.path().join("release.pub");
    assert!(
        run(&[
            "build",
            "verify",
            artifact.to_str().unwrap(),
            "--pub",
            pubkey.to_str().unwrap()
        ])
        .0
    );

    // Tampering must fail cleanly with the error code.
    let mut bytes = std::fs::read(&artifact).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(&artifact, &bytes).unwrap();
    let (ok, out) = run(&[
        "build",
        "verify",
        artifact.to_str().unwrap(),
        "--pub",
        pubkey.to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(out.contains("CAVS-E-SIGNATURE-INVALID"), "{out}");

    // Encrypt/decrypt round trip via the binary.
    let ckey = dir.path().join("content.key");
    let enc = dir.path().join("build.enc");
    let dec = dir.path().join("build.dec");
    assert!(run(&["build", "content-key", "--out", ckey.to_str().unwrap()]).0);
    assert!(
        run(&[
            "build",
            "encrypt",
            artifact.to_str().unwrap(),
            "--key",
            ckey.to_str().unwrap(),
            "--out",
            enc.to_str().unwrap(),
        ])
        .0
    );
    assert!(
        run(&[
            "build",
            "decrypt",
            enc.to_str().unwrap(),
            "--key",
            ckey.to_str().unwrap(),
            "--out",
            dec.to_str().unwrap(),
        ])
        .0
    );
    assert_eq!(
        std::fs::read(&artifact).unwrap(),
        std::fs::read(&dec).unwrap()
    );
}

#[test]
fn optimize_layout_writes_machine_plan() {
    let dir = tempfile::tempdir().unwrap();
    let (v1, _, shuffled) = pack_scenarios(dir.path());
    let plan_path = dir.path().join("layout.json");
    let (ok, _) = run(&[
        "optimize-layout",
        v1.to_str().unwrap(),
        shuffled.to_str().unwrap(),
        "--write-plan",
        plan_path.to_str().unwrap(),
    ]);
    assert!(ok);
    let plan: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan_path).unwrap()).unwrap();
    assert!(!plan["recommendations"].as_array().unwrap().is_empty());
    assert!(plan["note"].as_str().unwrap().contains("Advisory only"));
    assert!(!plan["general_rules"].as_array().unwrap().is_empty());
}
