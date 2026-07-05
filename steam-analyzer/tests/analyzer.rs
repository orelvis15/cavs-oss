//! End-to-end analyzer tests: build synthetic game builds and assert the
//! SteamPipe/CDC models and risk diagnostics behave as designed, plus the CI
//! gate exit codes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn prng(n: usize, seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; n];
    let mut s = seed;
    for b in out.iter_mut() {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (s >> 24) as u8;
    }
    out
}

fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

fn bin() -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    p.pop();
    p.push("cavs-steam");
    p
}

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), text)
}

/// Read a numeric field out of the JSON report.
fn field(report_dir: &Path, key: &str) -> serde_json::Value {
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report_dir.join("results.json")).unwrap())
            .unwrap();
    json[key].clone()
}

#[test]
fn localized_change_is_low_risk() {
    let dir = tempfile::tempdir().unwrap();
    let base = prng(20 * 1024 * 1024, 42);
    write(&dir.path().join("v1/Content/Paks/game.pak"), &base);
    // Localized 200 KiB edit; everything else stays 1 MiB-aligned.
    let mut v2 = base.clone();
    v2[5_000_000..5_000_000 + 200 * 1024].copy_from_slice(&prng(200 * 1024, 7));
    write(&dir.path().join("v2/Content/Paks/game.pak"), &v2);

    let out = dir.path().join("rep");
    let (code, _) = run(&[
        "compare",
        dir.path().join("v1").to_str().unwrap(),
        dir.path().join("v2").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    // A localized change patches efficiently: small steam update, not high risk.
    let steam = field(&out, "estimated_steam_update_bytes").as_u64().unwrap();
    assert!(steam < 4 * 1024 * 1024, "localized change should be small, got {steam}");
    assert_ne!(field(&out, "risk").as_str().unwrap(), "high");
}

#[test]
fn reorder_flags_high_risk_and_misalignment() {
    let dir = tempfile::tempdir().unwrap();
    let base = prng(20 * 1024 * 1024, 42);
    write(&dir.path().join("v1/Content/Paks/game.pak"), &base);
    // Insert 100 KiB at the front: shifts every byte -> no fixed chunk aligns.
    let mut v3 = prng(100 * 1024, 9);
    v3.extend_from_slice(&base);
    write(&dir.path().join("v3/Content/Paks/game.pak"), &v3);

    let out = dir.path().join("rep");
    run(&[
        "compare",
        dir.path().join("v1").to_str().unwrap(),
        dir.path().join("v3").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    // SteamPipe re-downloads almost everything; CAVS FastCDC would not.
    let steam = field(&out, "estimated_steam_update_bytes").as_u64().unwrap();
    let cdc = field(&out, "estimated_cdc_update_bytes").as_u64().unwrap();
    assert!(steam > 15 * 1024 * 1024, "reorder should blow up steam update, got {steam}");
    assert!(cdc < steam / 10, "CDC should stay tiny vs steam: cdc={cdc} steam={steam}");
    assert_eq!(field(&out, "risk").as_str().unwrap(), "high");
    let json = std::fs::read_to_string(out.join("results.json")).unwrap();
    assert!(json.contains("cdc_reuse_much_higher_than_fixed_reuse"));
    // The report artifacts exist.
    assert!(out.join("index.html").exists());
    assert!(out.join("summary.md").exists());
}

#[test]
fn ci_gate_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let base = prng(8 * 1024 * 1024, 1);
    write(&dir.path().join("v1/game.pak"), &base);
    let mut v3 = prng(64 * 1024, 2);
    v3.extend_from_slice(&base); // reorder -> large update, high risk
    write(&dir.path().join("v3/game.pak"), &v3);

    let out = dir.path().join("rep");
    // Budget far below the (full) reordered update -> hard fail (exit 2).
    let (code, log) = run(&[
        "ci",
        dir.path().join("v1").to_str().unwrap(),
        dir.path().join("v3").to_str().unwrap(),
        "--max-estimated-update",
        "1MiB",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "expected hard failure exit code:\n{log}");
    assert!(log.contains("exceeds budget") || log.contains("risk"));

    // Generous budget + only allow failing at >high: identical builds pass.
    write(&dir.path().join("same_a/game.pak"), &base);
    write(&dir.path().join("same_b/game.pak"), &base);
    let (code, _) = run(&[
        "ci",
        dir.path().join("same_a").to_str().unwrap(),
        dir.path().join("same_b").to_str().unwrap(),
        "--max-estimated-update",
        "100GiB",
        "--out",
        dir.path().join("rep2").to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "identical builds should pass CI");
}
