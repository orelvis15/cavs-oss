//! Payload classification (CAVS v2, P0-2).
//!
//! Before chunking, the packer inspects the payload — extension, magic
//! bytes, sampled entropy and a zstd compression probe — and produces a
//! [`PayloadProfile`] that drives profile selection: already-compressed
//! payloads prefer large fixed chunks (chunk-level zstd is skipped anyway),
//! update-heavy engine packs prefer content-defined chunking, and
//! metadata/text prefers small CDC chunks.

use crate::profile::ChunkProfile;
use std::path::Path;

/// What kind of payload a file looks like. Drives chunking heuristics only —
/// never correctness: reconstruction is byte-identical regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    GodotPck,
    UnityBundle,
    UnrealPak,
    UnrealUcas,
    ZipArchive,
    TarArchive,
    VideoSegment,
    ImageCompressed,
    AudioCompressed,
    MetadataText,
    RawBinary,
    Unknown,
}

impl PayloadKind {
    pub fn label(&self) -> &'static str {
        match self {
            PayloadKind::GodotPck => "godot-pck",
            PayloadKind::UnityBundle => "unity-bundle",
            PayloadKind::UnrealPak => "unreal-pak",
            PayloadKind::UnrealUcas => "unreal-ucas",
            PayloadKind::ZipArchive => "zip",
            PayloadKind::TarArchive => "tar",
            PayloadKind::VideoSegment => "video",
            PayloadKind::ImageCompressed => "image",
            PayloadKind::AudioCompressed => "audio",
            PayloadKind::MetadataText => "text",
            PayloadKind::RawBinary => "raw-binary",
            PayloadKind::Unknown => "unknown",
        }
    }
}

/// Classification result plus the measured signals behind it.
#[derive(Debug, Clone)]
pub struct PayloadProfile {
    pub kind: PayloadKind,
    /// Shannon entropy of the sampled bytes, in bits per byte (0..=8).
    pub entropy_score: f32,
    /// compressed/raw ratio of a zstd-3 probe over the samples (1.0 = no gain).
    pub zstd_sample_ratio: f32,
    pub likely_precompressed: bool,
    pub likely_update_heavy: bool,
    /// Candidate chunk profiles worth sweeping for this payload, best-first.
    pub recommended_profiles: Vec<ChunkProfile>,
}

/// Sample windows: up to 16 windows of 64 KiB spread evenly over the file.
const SAMPLE_WINDOW: usize = 64 * 1024;
const SAMPLE_WINDOWS: usize = 16;

/// Classify a payload from its path (extension), magic bytes and content
/// samples. `data` is the full file; only sampled windows are inspected.
pub fn classify(path: &Path, data: &[u8]) -> PayloadProfile {
    let kind = detect_kind(path, data);
    let samples = sample_windows(data);
    let entropy_score = shannon_entropy(&samples);
    let zstd_sample_ratio = zstd_probe_ratio(&samples);

    // High-entropy content that a zstd probe barely compresses is already
    // compressed (or encrypted): chunk-level zstd will be skipped and small
    // chunks only add manifest weight.
    let kind_precompressed = matches!(
        kind,
        PayloadKind::ZipArchive
            | PayloadKind::VideoSegment
            | PayloadKind::ImageCompressed
            | PayloadKind::AudioCompressed
    );
    let likely_precompressed =
        kind_precompressed || (entropy_score > 7.8 && zstd_sample_ratio > 0.99);

    // Engine packs and archives shift content between versions: FastCDC
    // keeps chunk boundaries stable under insertions/deletions.
    let likely_update_heavy = matches!(
        kind,
        PayloadKind::GodotPck
            | PayloadKind::UnityBundle
            | PayloadKind::UnrealPak
            | PayloadKind::UnrealUcas
            | PayloadKind::TarArchive
    );

    let recommended_profiles = if likely_precompressed {
        // No dedup or compression to win inside: minimise chunk count.
        vec![
            ChunkProfile::Fixed1M,
            ChunkProfile::Fixed512K,
            ChunkProfile::FastCdc256K,
        ]
    } else if likely_update_heavy {
        ChunkProfile::ALL.to_vec()
    } else if matches!(kind, PayloadKind::MetadataText) {
        vec![
            ChunkProfile::FastCdc16K,
            ChunkProfile::FastCdc32K,
            ChunkProfile::FastCdc64K,
            ChunkProfile::FastCdc128K,
            ChunkProfile::Fixed256K,
        ]
    } else {
        ChunkProfile::ALL.to_vec()
    };

    PayloadProfile {
        kind,
        entropy_score,
        zstd_sample_ratio,
        likely_precompressed,
        likely_update_heavy,
        recommended_profiles,
    }
}

fn detect_kind(path: &Path, data: &[u8]) -> PayloadKind {
    // Magic bytes first: they beat lying extensions.
    if data.len() >= 4 {
        match &data[..4] {
            b"GDPC" => return PayloadKind::GodotPck,
            b"PK\x03\x04" | b"PK\x05\x06" => return PayloadKind::ZipArchive,
            b"OggS" => return PayloadKind::AudioCompressed,
            b"\x89PNG" => return PayloadKind::ImageCompressed,
            _ => {}
        }
        if data.len() >= 8 && &data[..7] == b"UnityFS" {
            return PayloadKind::UnityBundle;
        }
        if data.len() >= 8 && &data[4..8] == b"ftyp" {
            return PayloadKind::VideoSegment;
        }
        if data[..3] == [0xFF, 0xD8, 0xFF] {
            return PayloadKind::ImageCompressed;
        }
        if data[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
            return PayloadKind::VideoSegment; // Matroska/WebM
        }
    }
    // tar has its magic at offset 257.
    if data.len() > 262 && &data[257..262] == b"ustar" {
        return PayloadKind::TarArchive;
    }

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pck" => PayloadKind::GodotPck,
        "bundle" | "assets" | "ress" => PayloadKind::UnityBundle,
        "pak" => PayloadKind::UnrealPak,
        "ucas" | "utoc" => PayloadKind::UnrealUcas,
        "zip" => PayloadKind::ZipArchive,
        "tar" => PayloadKind::TarArchive,
        "mp4" | "m4s" | "webm" | "mkv" | "mov" => PayloadKind::VideoSegment,
        "png" | "jpg" | "jpeg" | "webp" | "ktx2" | "basis" => PayloadKind::ImageCompressed,
        "ogg" | "mp3" | "opus" | "aac" | "flac" => PayloadKind::AudioCompressed,
        "json" | "txt" | "csv" | "xml" | "yaml" | "yml" | "toml" | "md" | "cfg" | "ini"
        | "tres" | "tscn" | "import" => PayloadKind::MetadataText,
        "wav" | "bmp" | "tga" | "bin" | "dat" => PayloadKind::RawBinary,
        _ => PayloadKind::Unknown,
    }
}

/// Up to [`SAMPLE_WINDOWS`] windows of [`SAMPLE_WINDOW`] bytes, spread evenly.
fn sample_windows(data: &[u8]) -> Vec<u8> {
    if data.len() <= SAMPLE_WINDOW * SAMPLE_WINDOWS {
        return data.to_vec();
    }
    let stride = data.len() / SAMPLE_WINDOWS;
    let mut out = Vec::with_capacity(SAMPLE_WINDOW * SAMPLE_WINDOWS);
    for i in 0..SAMPLE_WINDOWS {
        let start = i * stride;
        let end = (start + SAMPLE_WINDOW).min(data.len());
        out.extend_from_slice(&data[start..end]);
    }
    out
}

/// Shannon entropy in bits per byte (8.0 = uniformly random).
fn shannon_entropy(data: &[u8]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let len = data.len() as f64;
    let mut h = 0.0f64;
    for &c in &counts {
        if c > 0 {
            let p = c as f64 / len;
            h -= p * p.log2();
        }
    }
    h as f32
}

/// compressed/raw ratio of a zstd level-3 probe (1.0 when incompressible).
fn zstd_probe_ratio(samples: &[u8]) -> f32 {
    if samples.is_empty() {
        return 1.0;
    }
    match zstd::bulk::compress(samples, 3) {
        Ok(compressed) => (compressed.len() as f64 / samples.len() as f64).min(1.0) as f32,
        Err(_) => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    #[test]
    fn zeros_are_low_entropy_and_compressible() {
        let p = classify(&PathBuf::from("data.bin"), &vec![0u8; 512 * 1024]);
        assert!(p.entropy_score < 1.0);
        assert!(p.zstd_sample_ratio < 0.1);
        assert!(!p.likely_precompressed);
    }

    #[test]
    fn random_bytes_look_precompressed() {
        let p = classify(&PathBuf::from("blob"), &pseudo_random(1024 * 1024, 7));
        assert!(p.entropy_score > 7.8, "entropy {}", p.entropy_score);
        assert!(p.zstd_sample_ratio > 0.99);
        assert!(p.likely_precompressed);
        assert_eq!(p.recommended_profiles[0], ChunkProfile::Fixed1M);
    }

    #[test]
    fn godot_pck_magic_wins_over_extension() {
        let mut data = b"GDPC".to_vec();
        data.extend_from_slice(&vec![7u8; 4096]);
        let p = classify(&PathBuf::from("weird.dat"), &data);
        assert_eq!(p.kind, PayloadKind::GodotPck);
        assert!(p.likely_update_heavy);
    }

    #[test]
    fn extension_fallbacks() {
        let payload = vec![3u8; 4096];
        for (name, kind) in [
            ("a.pck", PayloadKind::GodotPck),
            ("b.pak", PayloadKind::UnrealPak),
            ("c.ucas", PayloadKind::UnrealUcas),
            ("d.json", PayloadKind::MetadataText),
            ("e.wav", PayloadKind::RawBinary),
        ] {
            assert_eq!(
                classify(&PathBuf::from(name), &payload).kind,
                kind,
                "{name}"
            );
        }
    }

    #[test]
    fn jpeg_magic_is_precompressed() {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0];
        data.extend_from_slice(&pseudo_random(256 * 1024, 3));
        let p = classify(&PathBuf::from("photo.raw"), &data);
        assert_eq!(p.kind, PayloadKind::ImageCompressed);
        assert!(p.likely_precompressed);
    }
}
