//! Thin wrappers around the ffmpeg/ffprobe/ffplay binaries.
//!
//! CAVS-1 deliberately does not reimplement codecs: encoding stays in mature
//! encoders, and CAVS packages the resulting CMAF/fMP4 segments.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

pub fn require(tool: &str) -> Result<()> {
    let ok = Command::new(tool)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        bail!(
            "`{tool}` not found or not runnable. Install it (e.g. `brew install ffmpeg`) \
             or use `cavs pack --raw` to pack file bytes without segmentation."
        );
    }
    Ok(())
}

/// Segment `input` into HLS/fMP4 (CMAF-style) inside `workdir`:
/// `init.mp4`, `seg_%05d.m4s` and `media.m3u8`. Returns true if stream-copy
/// was used, false if it had to transcode.
pub fn segment_to_cmaf(
    input: &Path,
    workdir: &Path,
    segment_time: f64,
    force_transcode: bool,
) -> Result<bool> {
    let input = input
        .canonicalize()
        .with_context(|| format!("input not found: {}", input.display()))?;

    let run = |codec_args: &[&str]| -> Result<bool> {
        let status = Command::new("ffmpeg")
            .current_dir(workdir)
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&input)
            .args(codec_args)
            .args([
                "-f",
                "hls",
                "-hls_time",
                &format!("{segment_time}"),
                "-hls_playlist_type",
                "vod",
                "-hls_segment_type",
                "fmp4",
                "-hls_segment_filename",
                "seg_%05d.m4s",
                "-hls_fmp4_init_filename",
                "init.mp4",
                "media.m3u8",
            ])
            .status()
            .context("failed to spawn ffmpeg")?;
        Ok(status.success())
    };

    if !force_transcode && run(&["-c", "copy"])? {
        return Ok(true);
    }
    // Stream copy failed or was skipped: normalize to H.264 + AAC, with
    // keyframes aligned to segment boundaries.
    let force_kf = format!("expr:gte(t,n_forced*{segment_time})");
    let transcode_args = [
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "21",
        "-force_key_frames",
        force_kf.as_str(),
        "-c:a",
        "aac",
        "-b:a",
        "128k",
    ];
    if run(&transcode_args)? {
        return Ok(false);
    }
    bail!("ffmpeg failed to segment {}", input.display());
}

/// Best-effort codec description like "h264+aac" via ffprobe.
pub fn probe_codecs(input: &Path) -> String {
    let probe = |selector: &str| -> Option<String> {
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                selector,
                "-show_entries",
                "stream=codec_name",
                "-of",
                "csv=p=0",
            ])
            .arg(input)
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s.lines().next().unwrap_or_default().to_string())
    };
    match (probe("v:0"), probe("a:0")) {
        (Some(v), Some(a)) => format!("{v}+{a}"),
        (Some(v), None) => v,
        (None, Some(a)) => a,
        (None, None) => "unknown".to_string(),
    }
}

pub fn play_file(path: &Path) -> Result<()> {
    require("ffplay")?;
    let status = Command::new("ffplay")
        .args(["-hide_banner", "-loglevel", "error", "-autoexit"])
        .arg(path)
        .status()
        .context("failed to spawn ffplay")?;
    if !status.success() {
        bail!("ffplay exited with an error");
    }
    Ok(())
}

/// Parse `#EXTINF` durations from an HLS media playlist, in order:
/// returns (segment_uri, duration_seconds) pairs.
pub fn parse_playlist(m3u8: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut pending: Option<f64> = None;
    for line in m3u8.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let dur = rest
                .split(',')
                .next()
                .and_then(|d| d.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            pending = Some(dur);
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(dur) = pending.take() {
                out.push((line.to_string(), dur));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_playlist;

    #[test]
    fn parses_extinf_pairs() {
        let playlist = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-MAP:URI=\"init.mp4\"\n\
                        #EXTINF:4.004000,\nseg_00000.m4s\n\
                        #EXTINF:2.500000,\nseg_00001.m4s\n#EXT-X-ENDLIST\n";
        let segs = parse_playlist(playlist);
        assert_eq!(
            segs,
            vec![
                ("seg_00000.m4s".to_string(), 4.004),
                ("seg_00001.m4s".to_string(), 2.5)
            ]
        );
    }
}
