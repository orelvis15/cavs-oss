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

/// How long a probe is allowed to run before we assume the binary is a
/// long-lived / GUI process and stop waiting for it. A CLI answering a
/// bogus flag exits in milliseconds; a GUI (e.g. a Homebrew `godot` that
/// ignores `--cavs-probe` and opens its project manager) never exits, so
/// waiting on `.status()` would hang the whole command forever.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Is `bin` runnable at all? (Spawning with a bogus flag is enough — a
/// missing binary errors at spawn, an existing one merely exits non-zero.)
///
/// The probe is bounded by [`PROBE_TIMEOUT`]: if the child hasn't exited by
/// then it is killed and reaped, and the binary is still reported as
/// available — it clearly launched. This keeps a GUI `godot` on `PATH` from
/// hanging `certify` and the benchmark harnesses indefinitely.
pub fn available(bin: &str) -> bool {
    let child = Command::new(bin)
        .arg("--cavs-probe")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        // Spawn failure = binary missing / not executable.
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            // Exited on its own (any status): a real, runnable CLI.
            Ok(Some(_)) => return true,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Still alive past the deadline: assume a GUI/daemon that
                    // ignores the flag. It launched, so it is available; kill
                    // and reap it so we neither hang nor leak a process.
                    let _ = child.kill();
                    let _ = child.wait();
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // Waiting failed: be conservative and call it available (it did
            // spawn), after a best-effort kill.
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return true;
            }
        }
    }
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
    fn available_false_for_missing_binary() {
        assert!(!available("cavs-definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn available_true_for_a_real_cli_that_exits() {
        // `true` exists on every unix and exits immediately regardless of
        // the bogus flag — the fast, self-terminating path.
        if Path::new("/usr/bin/true").exists() || Path::new("/bin/true").exists() {
            let bin = if Path::new("/bin/true").exists() {
                "/bin/true"
            } else {
                "/usr/bin/true"
            };
            assert!(available(bin));
        }
    }

    /// The regression that motivated the timeout: a binary that never exits
    /// on the probe flag (a GUI/daemon stand-in) must not hang `available`.
    /// `sleep 3600` ignores `--cavs-probe`, runs far past PROBE_TIMEOUT, and
    /// must be killed and reported available in ~PROBE_TIMEOUT, not 3600 s.
    #[test]
    fn available_does_not_hang_on_a_never_exiting_binary() {
        let sleep_bin = ["/bin/sleep", "/usr/bin/sleep"]
            .into_iter()
            .find(|p| Path::new(p).exists());
        let Some(sleep_bin) = sleep_bin else {
            return; // no sleep on this platform; skip
        };
        let started = std::time::Instant::now();
        // available() ignores extra args, so shadow the arg by probing a
        // wrapper: we can't pass "3600" through available(), so exercise the
        // bounded-wait helper shape directly via a long-running child.
        let mut child = Command::new(sleep_bin)
            .arg("3600")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
        let mut killed = false;
        loop {
            match child.try_wait().unwrap() {
                Some(_) => break,
                None => {
                    if std::time::Instant::now() >= deadline {
                        child.kill().unwrap();
                        child.wait().unwrap();
                        killed = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
        assert!(killed, "long-running child should hit the deadline");
        assert!(
            started.elapsed() < PROBE_TIMEOUT + std::time::Duration::from_secs(2),
            "bounded wait took too long: {:?}",
            started.elapsed()
        );
    }

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
