//! Measured execution of external benchmark tools: wall time, exit
//! status, captured stdout/stderr and (best-effort) peak RSS via the
//! platform's `/usr/bin/time` (`-l` on macOS reports bytes, `-v` on Linux
//! reports KiB). When `/usr/bin/time` is unavailable the run still works,
//! just without the memory figure.

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Default, serde::Serialize)]
pub struct MeasuredRun {
    pub wall_ms: u64,
    pub exit_ok: bool,
    pub exit_code: Option<i32>,
    pub peak_rss_mib: Option<f64>,
    #[serde(skip)]
    pub stdout: Vec<u8>,
    #[serde(skip)]
    pub stderr: String,
}

/// Run `bin args…`, capturing stdout (e.g. butler's JSON lines).
pub fn run_measured(bin: &str, args: &[&str], cwd: Option<&Path>) -> Result<MeasuredRun> {
    let time_flag = if cfg!(target_os = "macos") {
        Some("-l")
    } else if cfg!(target_os = "linux") {
        Some("-v")
    } else {
        None
    };
    let use_time = time_flag.is_some() && Path::new("/usr/bin/time").is_file();

    let started = std::time::Instant::now();
    let mut cmd = if use_time {
        let mut c = Command::new("/usr/bin/time");
        c.arg(time_flag.unwrap()).arg(bin).args(args);
        c
    } else {
        let mut c = Command::new(bin);
        c.args(args);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
    let wall_ms = started.elapsed().as_millis() as u64;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(MeasuredRun {
        wall_ms,
        exit_ok: output.status.success(),
        exit_code: output.status.code(),
        peak_rss_mib: parse_peak_rss(&stderr),
        stdout: output.stdout,
        stderr,
    })
}

/// Parse `time -l` (macOS, bytes) or `time -v` (GNU, KiB) output.
fn parse_peak_rss(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("maximum resident set size") {
            let digits: String = line.chars().filter(|c| c.is_ascii_digit()).collect();
            let value: f64 = digits.parse().ok()?;
            return Some(if lower.contains("kbytes") {
                value / 1024.0
            } else {
                value / (1024.0 * 1024.0)
            });
        }
    }
    None
}

/// Is `bin` runnable at all? (Spawning with a bogus flag is enough — a
/// missing binary errors at spawn, an existing one merely exits non-zero.)
pub fn available(bin: &str) -> bool {
    Command::new(bin)
        .arg("--cavs-probe")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// First line of `bin <flag>` output (version banners), if it runs.
pub fn version_line(bin: &str, flag: &str) -> Option<String> {
    let out = Command::new(bin).arg(flag).output().ok()?;
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    text.lines().next().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_parsing_handles_both_formats() {
        assert_eq!(
            parse_peak_rss("  134217728  maximum resident set size\n"),
            Some(128.0)
        );
        assert_eq!(
            parse_peak_rss("\tMaximum resident set size (kbytes): 2048\n"),
            Some(2.0)
        );
        assert_eq!(parse_peak_rss("no memory info"), None);
    }
}
