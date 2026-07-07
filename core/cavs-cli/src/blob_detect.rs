//! Detection of compressed / high-entropy blobs — the shape that defeats
//! block-level patching. A one-line source change can rewrite most of a
//! compressed archive's bytes, so both the chunk route and offline plans
//! degrade to near-full downloads on them. Detecting the shape lets
//! `cavs preview` warn the developer and lets the per-file strategy
//! optimizer route the file to a byte-level delta instead.

/// What a payload looks like from its magic bytes and entropy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlobShape {
    /// A recognized archive/compressed container (magic bytes matched).
    Archive,
    /// No known magic, but the content does not compress — encrypted,
    /// already-compressed or media data.
    HighEntropy,
    /// Ordinary patch-friendly content.
    Plain,
}

impl BlobShape {
    pub fn label(self) -> &'static str {
        match self {
            BlobShape::Archive => "archive",
            BlobShape::HighEntropy => "high-entropy",
            BlobShape::Plain => "plain",
        }
    }
}

/// (magic bytes, human name). Installers and self-extracting executables
/// begin as plain executables, so they are caught by entropy, not magic.
const MAGICS: &[(&[u8], &str)] = &[
    (b"PK\x03\x04", "zip"),
    (b"PK\x05\x06", "zip (empty)"),
    (&[0x1f, 0x8b], "gzip"),
    (&[0x28, 0xb5, 0x2f, 0xfd], "zstd"),
    (b"7z\xbc\xaf\x27\x1c", "7z"),
    (&[0xfd, b'7', b'z', b'X', b'Z', 0x00], "xz"),
    (b"BZh", "bzip2"),
    (b"Rar!\x1a\x07", "rar"),
    (b"LZIP", "lzip"),
    (b"\xce\xb2\xcf\x81", "brotli (framed)"),
];

/// Identify a recognized archive magic, if any.
pub fn archive_magic(bytes: &[u8]) -> Option<&'static str> {
    MAGICS
        .iter()
        .find(|(magic, _)| bytes.starts_with(magic))
        .map(|(_, name)| *name)
}

/// zstd-3 compression ratio of a bounded sample (1.0 = incompressible).
pub fn sample_compression_ratio(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let sample = &bytes[..bytes.len().min(256 * 1024)];
    zstd::bulk::compress(sample, 3)
        .map(|c| c.len() as f64 / sample.len() as f64)
        .unwrap_or(1.0)
}

/// Classify a payload. `Archive` beats `HighEntropy` beats `Plain`.
pub fn classify_blob(bytes: &[u8]) -> (BlobShape, Option<&'static str>) {
    if let Some(name) = archive_magic(bytes) {
        return (BlobShape::Archive, Some(name));
    }
    if bytes.len() >= 64 * 1024 && sample_compression_ratio(bytes) > 0.97 {
        return (BlobShape::HighEntropy, None);
    }
    (BlobShape::Plain, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magics_are_recognized() {
        assert_eq!(archive_magic(b"PK\x03\x04rest"), Some("zip"));
        assert_eq!(archive_magic(&[0x28, 0xb5, 0x2f, 0xfd, 1, 2]), Some("zstd"));
        assert_eq!(archive_magic(b"plain text"), None);
    }

    #[test]
    fn entropy_classification() {
        // Compressible: repeated pattern.
        let plain = b"hello world ".repeat(10_000);
        assert_eq!(classify_blob(&plain).0, BlobShape::Plain);

        // Incompressible: zstd-compressed data without its magic reaching
        // the classifier is caught by the ratio check.
        let noise = zstd::bulk::compress(&plain, 3).unwrap();
        let mut stripped = vec![0u8; 64 * 1024];
        let mut state = 0x9e3779b9u32;
        for b in stripped.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        assert_eq!(classify_blob(&stripped).0, BlobShape::HighEntropy);
        // With its magic intact it is an archive.
        assert_eq!(classify_blob(&noise).0, BlobShape::Archive);
    }
}
