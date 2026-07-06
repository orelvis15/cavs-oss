//! Binary manifest v2 (`CAVSMF2`) encoder/decoder.
//!
//! Wire layout (little-endian scalars, unsigned LEB128 varints):
//!
//! ```text
//! Header:
//!   magic:          8 bytes  "CAVSMF2\0"
//!   version_major:  u16      2
//!   version_minor:  u16      0
//!   flags:          u32      reserved (0)
//!   hash_alg:       u8       1 = BLAKE3-256
//!   section_count:  varuint  (max 64)
//!
//! Section table (section_count entries):
//!   kind:           varuint
//!   compression:    u8       0 = none, 1 = zstd
//!   offset:         varuint  into the data region
//!   stored_len:     varuint
//!   raw_len:        varuint
//!   hash:           [32]     BLAKE3 of the *raw* (uncompressed) section
//!
//! Data region: the sections' stored bytes, in table order.
//! ```
//!
//! Sections (unknown kinds are skipped for forward compatibility):
//!
//! - `AssetInfo` (1): asset, uuid, merkle root, optional signature and
//!   packer meta entries.
//! - `ChunkPlan` (2): tracks, segments and dictionary pins. Every chunk
//!   reference is a varint index into the chunk dictionary — the encoding
//!   win over JSON v1, which repeats a 64-char hex hash per reference.
//! - `ChunkDictionary` (3): unique chunk hashes as raw 32-byte BLAKE3
//!   plus their raw lengths. The first `chunk_table_count` entries are the
//!   container's chunk table in Merkle leaf order.
//!
//! Sections whose raw encoding reaches 32 KiB are zstd-compressed. The
//! decoder is strict: section hashes must verify, every section must be
//! consumed exactly, and all counts/lengths are validated against hard
//! limits before any allocation, so malformed or hostile input fails
//! cleanly instead of panicking or over-allocating.

use crate::varint::{read_varuint, write_varuint};
use crate::{ManifestError, MANIFEST_V2_MAGIC, MAX_MANIFEST_BYTES};
use cavs_hash::{from_hex, hash_chunk, to_hex, ChunkHash};
use cavs_proto::{ChunkRef, Manifest, ManifestSegment, ManifestTrack};
use std::collections::HashMap;

const VERSION_MAJOR: u16 = 2;
const VERSION_MINOR: u16 = 0;
const HASH_ALG_BLAKE3: u8 = 1;

const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_ZSTD: u8 = 1;

const SECTION_ASSET_INFO: u64 = 1;
const SECTION_CHUNK_PLAN: u64 = 2;
const SECTION_CHUNK_DICTIONARY: u64 = 3;

/// Sections at or above this raw size are stored zstd-compressed.
const COMPRESS_MIN_SECTION: usize = 32 * 1024;
const SECTION_ZSTD_LEVEL: i32 = 3;

/// Safety limits enforced while decoding untrusted input.
const MAX_SECTION_COUNT: u64 = 64;
const MAX_SECTION_RAW: u64 = MAX_MANIFEST_BYTES as u64;
/// A compressed section may not expand more than this factor.
const MAX_DECOMPRESS_RATIO: u64 = 100;
const MAX_STRING_LEN: u64 = 64 * 1024;
/// Dictionary entry floor on the wire (32-byte hash + 1-byte varint len):
/// bounds `Vec` pre-allocations driven by untrusted counts.
const DICT_ENTRY_MIN_BYTES: u64 = 33;

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Encode the runtime manifest as binary v2 bytes.
///
/// Fails only when the manifest itself is inconsistent (hashes that are not
/// valid 64-char hex, strings above the wire limit).
pub fn encode_manifest_v2(manifest: &Manifest) -> Result<Vec<u8>, ManifestError> {
    let dictionary = Dictionary::build(manifest)?;

    let sections: [(u64, Vec<u8>); 3] = [
        (SECTION_ASSET_INFO, encode_asset_info(manifest)?),
        (
            SECTION_CHUNK_PLAN,
            encode_chunk_plan(manifest, &dictionary)?,
        ),
        (SECTION_CHUNK_DICTIONARY, encode_dictionary(&dictionary)),
    ];

    // Compress large sections; keep small ones raw.
    let mut stored: Vec<(u64, u8, Vec<u8>, usize, ChunkHash)> = Vec::new();
    for (kind, raw) in sections {
        let hash = hash_chunk(&raw);
        let raw_len = raw.len();
        let (compression, bytes) = if raw_len >= COMPRESS_MIN_SECTION {
            let compressed =
                zstd::bulk::compress(&raw, SECTION_ZSTD_LEVEL).map_err(ManifestError::Zstd)?;
            if compressed.len() < raw_len {
                (COMPRESSION_ZSTD, compressed)
            } else {
                (COMPRESSION_NONE, raw)
            }
        } else {
            (COMPRESSION_NONE, raw)
        };
        stored.push((kind, compression, bytes, raw_len, hash));
    }

    let mut out = Vec::new();
    out.extend_from_slice(MANIFEST_V2_MAGIC);
    out.extend_from_slice(&VERSION_MAJOR.to_le_bytes());
    out.extend_from_slice(&VERSION_MINOR.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.push(HASH_ALG_BLAKE3);
    write_varuint(stored.len() as u64, &mut out);

    let mut offset = 0u64;
    for (kind, compression, bytes, raw_len, hash) in &stored {
        write_varuint(*kind, &mut out);
        out.push(*compression);
        write_varuint(offset, &mut out);
        write_varuint(bytes.len() as u64, &mut out);
        write_varuint(*raw_len as u64, &mut out);
        out.extend_from_slice(hash);
        offset += bytes.len() as u64;
    }
    for (_, _, bytes, _, _) in &stored {
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

/// Unique chunk hashes of a manifest, ordered: the chunk table first (in
/// Merkle leaf order), then any referenced hash the table does not cover.
struct Dictionary {
    hashes: Vec<ChunkHash>,
    lens: Vec<u32>,
    index_of: HashMap<ChunkHash, u32>,
    chunk_table_count: usize,
}

impl Dictionary {
    fn build(manifest: &Manifest) -> Result<Self, ManifestError> {
        let mut dict = Dictionary {
            hashes: Vec::with_capacity(manifest.chunk_table.len()),
            lens: Vec::new(),
            index_of: HashMap::with_capacity(manifest.chunk_table.len()),
            chunk_table_count: 0,
        };
        for hex in &manifest.chunk_table {
            dict.intern(hex, None)?;
        }
        dict.chunk_table_count = dict.hashes.len();
        for track in &manifest.tracks {
            for c in &track.init_chunks {
                dict.intern(&c.hash, Some(c.len))?;
            }
        }
        for segment in &manifest.segments {
            for c in &segment.chunks {
                dict.intern(&c.hash, Some(c.len))?;
            }
        }
        for hex in &manifest.dict {
            dict.intern(hex, None)?;
        }
        Ok(dict)
    }

    fn intern(&mut self, hex: &str, len: Option<u32>) -> Result<u32, ManifestError> {
        let hash =
            from_hex(hex).ok_or_else(|| ManifestError::Encode(format!("bad chunk hash {hex}")))?;
        let index = match self.index_of.get(&hash) {
            Some(&i) => i,
            None => {
                let i = self.hashes.len() as u32;
                self.hashes.push(hash);
                self.lens.push(0);
                self.index_of.insert(hash, i);
                i
            }
        };
        if let Some(len) = len {
            self.lens[index as usize] = len;
        }
        Ok(index)
    }

    fn index(&self, hex: &str) -> Result<u32, ManifestError> {
        let hash =
            from_hex(hex).ok_or_else(|| ManifestError::Encode(format!("bad chunk hash {hex}")))?;
        self.index_of
            .get(&hash)
            .copied()
            .ok_or_else(|| ManifestError::Encode(format!("hash not interned: {hex}")))
    }
}

fn encode_dictionary(dict: &Dictionary) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + dict.hashes.len() * 35);
    write_varuint(dict.hashes.len() as u64, &mut out);
    write_varuint(dict.chunk_table_count as u64, &mut out);
    for (hash, len) in dict.hashes.iter().zip(&dict.lens) {
        out.extend_from_slice(hash);
        write_varuint(*len as u64, &mut out);
    }
    out
}

fn encode_asset_info(manifest: &Manifest) -> Result<Vec<u8>, ManifestError> {
    let mut out = Vec::new();
    put_str(&mut out, &manifest.asset)?;
    put_str(&mut out, &manifest.asset_uuid)?;
    put_str(&mut out, &manifest.merkle_root)?;
    put_opt_str(&mut out, manifest.signature.as_deref())?;
    put_opt_str(&mut out, manifest.signer_pubkey.as_deref())?;
    write_varuint(manifest.meta.len() as u64, &mut out);
    for (key, value) in &manifest.meta {
        put_str(&mut out, key)?;
        put_str(&mut out, value)?;
    }
    Ok(out)
}

fn encode_chunk_plan(manifest: &Manifest, dict: &Dictionary) -> Result<Vec<u8>, ManifestError> {
    let mut out = Vec::new();
    write_varuint(manifest.tracks.len() as u64, &mut out);
    for track in &manifest.tracks {
        write_varuint(track.track_id as u64, &mut out);
        put_str(&mut out, &track.kind)?;
        put_str(&mut out, &track.codec)?;
        put_str(&mut out, &track.name)?;
        write_varuint(track.timescale as u64, &mut out);
        write_varuint(track.init_chunks.len() as u64, &mut out);
        for c in &track.init_chunks {
            write_varuint(dict.index(&c.hash)? as u64, &mut out);
        }
    }
    write_varuint(manifest.segments.len() as u64, &mut out);
    for segment in &manifest.segments {
        write_varuint(segment.segment_id, &mut out);
        write_varuint(segment.track_id as u64, &mut out);
        write_varuint(segment.pts_start, &mut out);
        write_varuint(segment.duration as u64, &mut out);
        out.push(segment.random_access as u8);
        write_varuint(segment.chunks.len() as u64, &mut out);
        for c in &segment.chunks {
            write_varuint(dict.index(&c.hash)? as u64, &mut out);
        }
    }
    write_varuint(manifest.dict.len() as u64, &mut out);
    for hex in &manifest.dict {
        write_varuint(dict.index(hex)? as u64, &mut out);
    }
    Ok(out)
}

fn put_str(out: &mut Vec<u8>, s: &str) -> Result<(), ManifestError> {
    if s.len() as u64 > MAX_STRING_LEN {
        return Err(ManifestError::Encode(format!(
            "string of {} bytes exceeds the wire limit",
            s.len()
        )));
    }
    write_varuint(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

fn put_opt_str(out: &mut Vec<u8>, s: Option<&str>) -> Result<(), ManifestError> {
    match s {
        Some(s) => {
            out.push(1);
            put_str(out, s)
        }
        None => {
            out.push(0);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Decode binary v2 bytes back into the runtime manifest.
pub fn decode_manifest_v2(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let mut input = bytes
        .strip_prefix(MANIFEST_V2_MAGIC.as_slice())
        .ok_or(ManifestError::UnknownFormat)?;

    let version_major = take_u16(&mut input)?;
    let _version_minor = take_u16(&mut input)?;
    if version_major != VERSION_MAJOR {
        return Err(ManifestError::UnsupportedVersion(version_major));
    }
    let _flags = take_u32(&mut input)?;
    let hash_alg = take_u8(&mut input)?;
    if hash_alg != HASH_ALG_BLAKE3 {
        return Err(ManifestError::Malformed("hash algorithm"));
    }
    let section_count = read_varuint(&mut input)?;
    if section_count > MAX_SECTION_COUNT {
        return Err(ManifestError::OutOfBounds("section count"));
    }

    struct SectionRef {
        kind: u64,
        compression: u8,
        offset: u64,
        stored_len: u64,
        raw_len: u64,
        hash: ChunkHash,
    }
    let mut table = Vec::with_capacity(section_count as usize);
    for _ in 0..section_count {
        table.push(SectionRef {
            kind: read_varuint(&mut input)?,
            compression: take_u8(&mut input)?,
            offset: read_varuint(&mut input)?,
            stored_len: read_varuint(&mut input)?,
            raw_len: read_varuint(&mut input)?,
            hash: take_hash(&mut input)?,
        });
    }
    let data = input;

    let mut asset_info: Option<Vec<u8>> = None;
    let mut chunk_plan: Option<Vec<u8>> = None;
    let mut dictionary: Option<Vec<u8>> = None;
    for section in &table {
        let slot = match section.kind {
            SECTION_ASSET_INFO => &mut asset_info,
            SECTION_CHUNK_PLAN => &mut chunk_plan,
            SECTION_CHUNK_DICTIONARY => &mut dictionary,
            // Unknown sections are legal (future extensions): skip them.
            _ => continue,
        };
        if slot.is_some() {
            return Err(ManifestError::DuplicateSection(section.kind));
        }

        if section.offset > data.len() as u64
            || section.stored_len > data.len() as u64 - section.offset
        {
            return Err(ManifestError::OutOfBounds("section bounds"));
        }
        if section.raw_len > MAX_SECTION_RAW {
            return Err(ManifestError::OutOfBounds("section raw length"));
        }
        let stored = &data[section.offset as usize..(section.offset + section.stored_len) as usize];
        let raw = match section.compression {
            COMPRESSION_NONE => {
                if section.raw_len != section.stored_len {
                    return Err(ManifestError::Malformed("uncompressed section lengths"));
                }
                stored.to_vec()
            }
            COMPRESSION_ZSTD => {
                // Decompression-bomb guard: the header's raw_len is bounded
                // both absolutely and relative to the stored bytes before
                // any allocation happens.
                if section.raw_len > section.stored_len.saturating_mul(MAX_DECOMPRESS_RATIO) {
                    return Err(ManifestError::OutOfBounds("section decompressed size"));
                }
                let raw = zstd::bulk::decompress(stored, section.raw_len as usize)
                    .map_err(ManifestError::Zstd)?;
                if raw.len() as u64 != section.raw_len {
                    return Err(ManifestError::Malformed("section decompressed length"));
                }
                raw
            }
            _ => return Err(ManifestError::Malformed("section compression")),
        };
        if hash_chunk(&raw) != section.hash {
            return Err(ManifestError::SectionHashMismatch(section.kind));
        }
        *slot = Some(raw);
    }

    let dictionary = dictionary.ok_or(ManifestError::MissingSection("ChunkDictionary"))?;
    let asset_info = asset_info.ok_or(ManifestError::MissingSection("AssetInfo"))?;
    let chunk_plan = chunk_plan.ok_or(ManifestError::MissingSection("ChunkPlan"))?;

    let (hashes, lens, chunk_table_count) = decode_dictionary(&dictionary)?;
    // Hex-encode each dictionary hash once; plan references clone the
    // prepared string instead of re-encoding per logical chunk.
    let hex: Vec<String> = hashes.iter().map(to_hex).collect();
    let mut manifest = decode_asset_info(&asset_info)?;
    manifest.chunk_table = hex[..chunk_table_count].to_vec();
    decode_chunk_plan(&chunk_plan, &hex, &lens, &mut manifest)?;
    Ok(manifest)
}

fn decode_dictionary(mut input: &[u8]) -> Result<(Vec<ChunkHash>, Vec<u32>, usize), ManifestError> {
    let input = &mut input;
    let count = read_varuint(input)?;
    let chunk_table_count = read_varuint(input)?;
    if chunk_table_count > count {
        return Err(ManifestError::Malformed("chunk table count"));
    }
    // Each entry needs at least 33 bytes: an inflated count cannot ask for
    // more memory than the section itself provides.
    if count > input.len() as u64 / DICT_ENTRY_MIN_BYTES + 1 {
        return Err(ManifestError::OutOfBounds("dictionary count"));
    }
    let mut hashes = Vec::with_capacity(count as usize);
    let mut lens = Vec::with_capacity(count as usize);
    for _ in 0..count {
        hashes.push(take_hash(input)?);
        let len = read_varuint(input)?;
        if len > u32::MAX as u64 {
            return Err(ManifestError::OutOfBounds("chunk length"));
        }
        lens.push(len as u32);
    }
    ensure_consumed(input, "ChunkDictionary")?;
    Ok((hashes, lens, chunk_table_count as usize))
}

fn decode_asset_info(mut input: &[u8]) -> Result<Manifest, ManifestError> {
    let input = &mut input;
    let asset = take_str(input)?;
    let asset_uuid = take_str(input)?;
    let merkle_root = take_str(input)?;
    let signature = take_opt_str(input)?;
    let signer_pubkey = take_opt_str(input)?;
    let meta_count = read_varuint(input)?;
    if meta_count > input.len() as u64 / 2 + 1 {
        return Err(ManifestError::OutOfBounds("meta count"));
    }
    let mut meta = Vec::with_capacity(meta_count as usize);
    for _ in 0..meta_count {
        let key = take_str(input)?;
        let value = take_str(input)?;
        meta.push((key, value));
    }
    ensure_consumed(input, "AssetInfo")?;
    Ok(Manifest {
        asset,
        asset_uuid,
        tracks: Vec::new(),
        segments: Vec::new(),
        dict: Vec::new(),
        chunk_table: Vec::new(),
        merkle_root,
        signature,
        signer_pubkey,
        meta,
    })
}

fn decode_chunk_plan(
    mut input: &[u8],
    hex: &[String],
    lens: &[u32],
    manifest: &mut Manifest,
) -> Result<(), ManifestError> {
    let input = &mut input;
    let chunk_ref = |input: &mut &[u8]| -> Result<ChunkRef, ManifestError> {
        let index = read_varuint(input)?;
        let hash = hex
            .get(index as usize)
            .ok_or(ManifestError::OutOfBounds("dictionary index"))?;
        Ok(ChunkRef {
            hash: hash.clone(),
            len: lens[index as usize],
        })
    };
    let chunk_refs = |input: &mut &[u8]| -> Result<Vec<ChunkRef>, ManifestError> {
        let count = read_varuint(input)?;
        if count > input.len() as u64 + 1 {
            return Err(ManifestError::OutOfBounds("chunk ref count"));
        }
        (0..count).map(|_| chunk_ref(input)).collect()
    };

    let track_count = read_varuint(input)?;
    if track_count > input.len() as u64 / 4 + 1 {
        return Err(ManifestError::OutOfBounds("track count"));
    }
    for _ in 0..track_count {
        manifest.tracks.push(ManifestTrack {
            track_id: take_varu32(input, "track id")?,
            kind: take_str(input)?,
            codec: take_str(input)?,
            name: take_str(input)?,
            timescale: take_varu32(input, "timescale")?,
            init_chunks: chunk_refs(input)?,
        });
    }

    let segment_count = read_varuint(input)?;
    if segment_count > input.len() as u64 / 4 + 1 {
        return Err(ManifestError::OutOfBounds("segment count"));
    }
    for _ in 0..segment_count {
        manifest.segments.push(ManifestSegment {
            segment_id: read_varuint(input)?,
            track_id: take_varu32(input, "track id")?,
            pts_start: read_varuint(input)?,
            duration: take_varu32(input, "duration")?,
            random_access: match take_u8(input)? {
                0 => false,
                1 => true,
                _ => return Err(ManifestError::Malformed("random access flag")),
            },
            chunks: chunk_refs(input)?,
        });
    }

    let dict_count = read_varuint(input)?;
    if dict_count > input.len() as u64 + 1 {
        return Err(ManifestError::OutOfBounds("dict pin count"));
    }
    for _ in 0..dict_count {
        let index = read_varuint(input)?;
        let hash = hex
            .get(index as usize)
            .ok_or(ManifestError::OutOfBounds("dictionary index"))?;
        manifest.dict.push(hash.clone());
    }
    ensure_consumed(input, "ChunkPlan")
}

// ---------------------------------------------------------------------------
// Primitive readers (strict: every take validates remaining bytes).
// ---------------------------------------------------------------------------

fn take_bytes<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], ManifestError> {
    if input.len() < n {
        return Err(ManifestError::Truncated("manifest bytes"));
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

fn take_u8(input: &mut &[u8]) -> Result<u8, ManifestError> {
    Ok(take_bytes(input, 1)?[0])
}

fn take_u16(input: &mut &[u8]) -> Result<u16, ManifestError> {
    Ok(u16::from_le_bytes(
        take_bytes(input, 2)?.try_into().unwrap(),
    ))
}

fn take_u32(input: &mut &[u8]) -> Result<u32, ManifestError> {
    Ok(u32::from_le_bytes(
        take_bytes(input, 4)?.try_into().unwrap(),
    ))
}

fn take_hash(input: &mut &[u8]) -> Result<ChunkHash, ManifestError> {
    Ok(take_bytes(input, 32)?.try_into().unwrap())
}

fn take_varu32(input: &mut &[u8], what: &'static str) -> Result<u32, ManifestError> {
    let value = read_varuint(input)?;
    u32::try_from(value).map_err(|_| ManifestError::OutOfBounds(what))
}

fn take_str(input: &mut &[u8]) -> Result<String, ManifestError> {
    let len = read_varuint(input)?;
    if len > MAX_STRING_LEN {
        return Err(ManifestError::OutOfBounds("string length"));
    }
    let bytes = take_bytes(input, len as usize)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| ManifestError::Malformed("utf-8 string"))
}

fn take_opt_str(input: &mut &[u8]) -> Result<Option<String>, ManifestError> {
    match take_u8(input)? {
        0 => Ok(None),
        1 => Ok(Some(take_str(input)?)),
        _ => Err(ManifestError::Malformed("option tag")),
    }
}

fn ensure_consumed(input: &[u8], section: &'static str) -> Result<(), ManifestError> {
    if input.is_empty() {
        Ok(())
    } else {
        Err(ManifestError::Malformed(section))
    }
}
