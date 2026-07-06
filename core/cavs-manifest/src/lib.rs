//! CAVS manifest formats (v0.3.0 compact manifest).
//!
//! Two wire formats carry the same runtime model, [`cavs_proto::Manifest`]:
//!
//! - **JSON v1** — the original human-readable control-plane manifest.
//!   Stays supported forever as debug export, compatibility input and
//!   migration fallback.
//! - **Binary v2** (`CAVSMF2`) — a compact sectioned encoding: BLAKE3
//!   hashes as raw bytes deduplicated through a chunk dictionary, varint
//!   integers, chunk references as dictionary indexes, optional zstd
//!   section compression and per-section BLAKE3 integrity.
//!
//! [`read_manifest`] detects the format from the bytes themselves, so
//! servers and clients never branch on out-of-band hints. Everything
//! downstream keeps consuming the normalized [`Manifest`].

mod v2;
pub mod varint;

pub use v2::{decode_manifest_v2, encode_manifest_v2};

use cavs_proto::Manifest;

/// Magic prefix of a binary v2 manifest.
pub const MANIFEST_V2_MAGIC: &[u8; 8] = b"CAVSMF2\0";
/// Content type served/requested for binary v2 manifests over HTTP.
pub const MANIFEST_V2_CONTENT_TYPE: &str = "application/vnd.cavs.manifest-v2";

/// Hard ceiling on manifest input size, before any parsing.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormat {
    JsonV1,
    BinaryV2,
}

impl ManifestFormat {
    pub fn label(&self) -> &'static str {
        match self {
            ManifestFormat::JsonV1 => "json-v1",
            ManifestFormat::BinaryV2 => "binary-v2",
        }
    }
}

/// A decoded manifest plus the wire format it arrived in.
#[derive(Debug, Clone)]
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub format: ManifestFormat,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest bytes match no known format")]
    UnknownFormat,
    #[error("manifest larger than the configured limit")]
    TooLarge,
    #[error("unsupported binary manifest version {0}")]
    UnsupportedVersion(u16),
    #[error("truncated or malformed {0}")]
    Truncated(&'static str),
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("varint is overlong")]
    VarintOverlong,
    #[error("varint exceeds u64")]
    VarintOverflow,
    #[error("value out of bounds for {0}")]
    OutOfBounds(&'static str),
    #[error("section {0} hash mismatch (corrupted manifest)")]
    SectionHashMismatch(u64),
    #[error("missing required section {0}")]
    MissingSection(&'static str),
    #[error("duplicate section kind {0}")]
    DuplicateSection(u64),
    #[error("invalid JSON manifest: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zstd error: {0}")]
    Zstd(std::io::Error),
    #[error("cannot encode manifest: {0}")]
    Encode(String),
}

/// Decode a manifest in either wire format, detected from the bytes.
///
/// Binary v2 is recognized by its magic; anything that looks like JSON is
/// parsed as v1. Everything else is [`ManifestError::UnknownFormat`].
pub fn read_manifest(bytes: &[u8]) -> Result<LoadedManifest, ManifestError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    if bytes.starts_with(MANIFEST_V2_MAGIC) {
        return Ok(LoadedManifest {
            manifest: decode_manifest_v2(bytes)?,
            format: ManifestFormat::BinaryV2,
        });
    }
    if looks_like_json(bytes) {
        return Ok(LoadedManifest {
            manifest: serde_json::from_slice(bytes)?,
            format: ManifestFormat::JsonV1,
        });
    }
    Err(ManifestError::UnknownFormat)
}

fn looks_like_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'{')
}

/// Build the runtime manifest of a packed `.cavs` file, exactly as
/// `cavs-server` announces it for file-served assets. Used by the CLI
/// (`cavs manifest export|bench`) to work on containers without a server.
pub fn manifest_from_reader(
    reader: &cavs_format::Reader,
    asset_name: &str,
) -> Result<Manifest, cavs_format::FormatError> {
    use cavs_format::SEGMENT_FLAG_RANDOM_ACCESS;
    use cavs_hash::to_hex;

    let chunks = reader.chunks();
    let chunk_ref = |idx: &u32| -> Result<cavs_proto::ChunkRef, cavs_format::FormatError> {
        let rec = chunks
            .get(*idx as usize)
            .ok_or(cavs_format::FormatError::ChunkIndexOutOfRange(*idx))?;
        Ok(cavs_proto::ChunkRef {
            hash: to_hex(&rec.hash),
            len: rec.len_raw,
        })
    };

    let signature = reader
        .embedded_signature()
        .map(|(sig, pk)| (hex_of(&sig), hex_of(&pk)));

    Ok(Manifest {
        asset: asset_name.to_string(),
        asset_uuid: hex_of(&reader.superblock().asset_uuid),
        tracks: reader
            .tracks()
            .iter()
            .map(|t| {
                Ok(cavs_proto::ManifestTrack {
                    track_id: t.track_id,
                    kind: t.kind.label().to_string(),
                    codec: t.codec.clone(),
                    name: t.name.clone(),
                    timescale: t.timescale,
                    init_chunks: t
                        .init_chunks
                        .iter()
                        .map(chunk_ref)
                        .collect::<Result<Vec<_>, cavs_format::FormatError>>()?,
                })
            })
            .collect::<Result<Vec<_>, cavs_format::FormatError>>()?,
        segments: reader
            .segments()
            .iter()
            .map(|s| {
                Ok(cavs_proto::ManifestSegment {
                    segment_id: s.segment_id,
                    track_id: s.track_id,
                    pts_start: s.pts_start,
                    duration: s.duration,
                    random_access: s.flags & SEGMENT_FLAG_RANDOM_ACCESS != 0,
                    chunks: s
                        .chunks
                        .iter()
                        .map(chunk_ref)
                        .collect::<Result<Vec<_>, cavs_format::FormatError>>()?,
                })
            })
            .collect::<Result<Vec<_>, cavs_format::FormatError>>()?,
        dict: reader
            .dict()
            .iter()
            .map(|i| chunk_ref(i).map(|c| c.hash))
            .collect::<Result<Vec<_>, cavs_format::FormatError>>()?,
        chunk_table: chunks.iter().map(|c| to_hex(&c.hash)).collect(),
        merkle_root: to_hex(&reader.integrity().merkle_root),
        signature: signature.as_ref().map(|(sig, _)| sig.clone()),
        signer_pubkey: signature.as_ref().map(|(_, pk)| pk.clone()),
        meta: reader.meta().to_vec(),
    })
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
