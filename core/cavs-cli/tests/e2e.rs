//! End-to-end tests driving the `cavs` binary.
//!
//! The raw-mode tests always run. The video-mode test runs only when ffmpeg
//! is available on PATH (it is skipped otherwise, not failed).

use std::path::{Path, PathBuf};
use std::process::Command;

fn cavs_bin() -> PathBuf {
    // target/debug/cavs next to the test binary's directory.
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

fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut state = seed;
    for b in out.iter_mut() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (state >> 24) as u8;
    }
    out
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn raw_pack_unpack_roundtrip_and_dedup() {
    let dir = tempfile::tempdir().unwrap();

    // file_b shares a large prefix region with file_a (shifted): CDC dedup food.
    let file_a = pseudo_random(2_000_000, 42);
    let mut file_b = b"HEADER-INSERTED-".to_vec();
    file_b.extend_from_slice(&file_a);
    let a_path = dir.path().join("asset_a.bin");
    let b_path = dir.path().join("asset_b.bin");
    std::fs::write(&a_path, &file_a).unwrap();
    std::fs::write(&b_path, &file_b).unwrap();

    let cavs = dir.path().join("assets.cavs");
    let (ok, out) = run(&[
        "pack",
        "--raw",
        a_path.to_str().unwrap(),
        b_path.to_str().unwrap(),
        "-o",
        cavs.to_str().unwrap(),
    ]);
    assert!(ok, "pack failed:\n{out}");
    assert!(out.contains("savings"), "missing stats output:\n{out}");

    let (ok, out) = run(&["verify", cavs.to_str().unwrap()]);
    assert!(ok, "verify failed:\n{out}");
    assert!(out.contains("OK:"), "unexpected verify output:\n{out}");

    let (ok, out) = run(&["info", cavs.to_str().unwrap()]);
    assert!(ok, "info failed:\n{out}");
    assert!(out.contains("CAVS-1.0"), "unexpected info output:\n{out}");

    let outdir = dir.path().join("restored");
    let (ok, out) = run(&[
        "unpack",
        cavs.to_str().unwrap(),
        "-o",
        outdir.to_str().unwrap(),
    ]);
    assert!(ok, "unpack failed:\n{out}");

    // Byte-for-byte reconstruction.
    assert_eq!(std::fs::read(outdir.join("asset_a.bin")).unwrap(), file_a);
    assert_eq!(std::fs::read(outdir.join("asset_b.bin")).unwrap(), file_b);

    // Shifted duplicate content must have deduped: the .cavs must be much
    // smaller than the ~4 MB of logical input.
    let cavs_size = std::fs::metadata(&cavs).unwrap().len();
    let logical: u64 = (file_a.len() + file_b.len()) as u64;
    assert!(
        cavs_size < logical * 3 / 4,
        "expected CDC dedup to save >25%: cavs={cavs_size} logical={logical}"
    );
}

#[test]
fn identical_file_dedupes_completely() {
    let dir = tempfile::tempdir().unwrap();
    let data = pseudo_random(1_500_000, 7);
    let p1 = dir.path().join("one.bin");
    let p2 = dir.path().join("two.bin");
    std::fs::write(&p1, &data).unwrap();
    std::fs::write(&p2, &data).unwrap();

    let cavs = dir.path().join("dup.cavs");
    let (ok, out) = run(&[
        "pack",
        "--raw",
        "--no-compress",
        p1.to_str().unwrap(),
        p2.to_str().unwrap(),
        "-o",
        cavs.to_str().unwrap(),
    ]);
    assert!(ok, "pack failed:\n{out}");

    // Two identical files: stored payload ~= one copy.
    let cavs_size = std::fs::metadata(&cavs).unwrap().len();
    assert!(
        (cavs_size as usize) < data.len() + data.len() / 4,
        "second copy should be ~free: cavs={cavs_size}, one copy={}",
        data.len()
    );
}

#[test]
fn video_pack_unpack_verify() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();

    // Generate a small synthetic test clip (video+audio).
    let clip = dir.path().join("clip.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=duration=6:size=640x360:rate=30",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=6"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&clip)
        .status()
        .unwrap()
        .success();
    assert!(ok, "could not generate test clip");

    let cavs = dir.path().join("clip.cavs");
    let (ok, out) = run(&[
        "pack",
        clip.to_str().unwrap(),
        "-o",
        cavs.to_str().unwrap(),
        "--segment-time",
        "2",
    ]);
    assert!(ok, "video pack failed:\n{out}");

    let (ok, out) = run(&["verify", cavs.to_str().unwrap()]);
    assert!(ok, "verify failed:\n{out}");

    let outdir = dir.path().join("restored");
    let (ok, out) = run(&[
        "unpack",
        cavs.to_str().unwrap(),
        "-o",
        outdir.to_str().unwrap(),
    ]);
    assert!(ok, "unpack failed:\n{out}");

    // HLS layout present and the combined mp4 decodes cleanly end-to-end.
    assert!(outdir.join("clip/init.mp4").exists());
    assert!(outdir.join("clip/media.m3u8").exists());
    assert!(outdir.join("clip/seg_00000.m4s").exists());
    let combined = outdir.join("clip.mp4");
    assert!(combined.exists());

    let decode_ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-xerror", "-i"])
        .arg(&combined)
        .args(["-f", "null", "-"])
        .status()
        .unwrap()
        .success();
    assert!(decode_ok, "reconstructed mp4 must decode without errors");

    check_probe_duration(&combined);
}

fn check_probe_duration(path: &Path) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .unwrap();
    let dur: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0);
    assert!(
        (dur - 6.0).abs() < 1.0,
        "expected ~6s clip after reconstruction, got {dur}s"
    );
}
