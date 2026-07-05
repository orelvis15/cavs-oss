//! Reader / verifier for CAVS-1 files.

use crate::wire::Cursor;
use crate::{
    ChunkRecord, FormatError, Integrity, Result, SectionEntry, SectionType, SegmentRecord,
    Superblock, TrackKind, TrackRecord, CHUNK_FLAG_ZSTD, MAGIC, SECTION_DIR_ENTRY_LEN,
    SUPERBLOCK_LEN, VERSION_MAJOR,
};
use cavs_hash::{content_signature_message, hash_chunk, merkle_root, Hasher};
use std::fs::File;
use std::io::{Read as _, Seek, SeekFrom};
use std::path::Path;

pub struct Reader {
    file: File,
    superblock: Superblock,
    sections: Vec<SectionEntry>,
    tracks: Vec<TrackRecord>,
    dict: Vec<u32>,
    chunks: Vec<ChunkRecord>,
    segments: Vec<SegmentRecord>,
    meta: Vec<(String, String)>,
    integrity: Integrity,
    data_offset: u64,
    data_len: u64,
}

/// Result of a full-file verification pass.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub chunks_verified: u64,
    pub bytes_verified: u64,
    pub merkle_ok: bool,
    pub data_section_ok: bool,
}

/// Outcome of a content-signature check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// No signature embedded.
    Unsigned,
    /// Signature verified; the signer's Ed25519 public key.
    Valid([u8; 32]),
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        // Real file length: every offset/length parsed below is validated
        // against it so a crafted header can never trigger a huge allocation.
        let file_len = file.metadata()?.len();

        let mut sb_bytes = [0u8; SUPERBLOCK_LEN as usize];
        file.read_exact(&mut sb_bytes)?;
        if sb_bytes[0..4] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let mut cur = Cursor::new(&sb_bytes[4..], "superblock");
        let superblock = Superblock {
            version_major: cur.u16()?,
            version_minor: cur.u16()?,
            feature_flags: cur.u32()?,
            hash_algo: cur.u8()?,
            compression_algo: cur.u8()?,
            asset_uuid: {
                cur.u16()?; // reserved
                let mut uuid = [0u8; 16];
                for b in uuid.iter_mut() {
                    *b = cur.u8()?;
                }
                uuid
            },
            timescale: cur.u32()?,
            section_count: cur.u32()?,
            section_dir_offset: cur.u64()?,
            file_size: cur.u64()?,
        };
        if superblock.version_major != VERSION_MAJOR {
            return Err(FormatError::UnsupportedVersion(superblock.version_major));
        }

        // Section directory. Validate offset + size against the file before
        // allocating, so a bogus section_count can't ask for gigabytes.
        let dir_len = superblock.section_count as u64 * SECTION_DIR_ENTRY_LEN as u64;
        if superblock.section_dir_offset > file_len
            || dir_len > file_len - superblock.section_dir_offset
        {
            return Err(FormatError::Malformed("section directory"));
        }
        file.seek(SeekFrom::Start(superblock.section_dir_offset))?;
        let mut dir_bytes = vec![0u8; dir_len as usize];
        file.read_exact(&mut dir_bytes)?;
        let mut cur = Cursor::new(&dir_bytes, "section directory");
        let mut sections = Vec::with_capacity(superblock.section_count as usize);
        for _ in 0..superblock.section_count {
            let ty_raw = cur.u32()?;
            let section_type = SectionType::from_u32(ty_raw).ok_or(FormatError::UnknownValue {
                what: "section type",
                value: ty_raw,
            })?;
            sections.push(SectionEntry {
                section_type,
                offset: cur.u64()?,
                length: cur.u64()?,
                hash: cur.hash()?,
            });
        }

        let read_section = |file: &mut File, ty: SectionType| -> Result<Vec<u8>> {
            let entry = sections
                .iter()
                .find(|s| s.section_type == ty)
                .ok_or(FormatError::MissingSection(ty))?;
            if entry.offset > file_len || entry.length > file_len - entry.offset {
                return Err(FormatError::Malformed("section bounds"));
            }
            file.seek(SeekFrom::Start(entry.offset))?;
            let mut buf = vec![0u8; entry.length as usize];
            file.read_exact(&mut buf)?;
            // Table sections are small; verify their hash eagerly.
            if hash_chunk(&buf) != entry.hash {
                return Err(FormatError::SectionHashMismatch(ty));
            }
            Ok(buf)
        };

        let tracks = decode_tracks(&read_section(&mut file, SectionType::Tracks)?)?;
        let dict = decode_dict(&read_section(&mut file, SectionType::Dict)?)?;
        let chunks = decode_chunks(&read_section(&mut file, SectionType::Chunks)?)?;
        let segments = decode_segments(&read_section(&mut file, SectionType::Segments)?)?;
        let meta = decode_meta(&read_section(&mut file, SectionType::Meta)?)?;
        let integrity = decode_integrity(&read_section(&mut file, SectionType::Integrity)?)?;

        let data_entry = sections
            .iter()
            .find(|s| s.section_type == SectionType::Data)
            .ok_or(FormatError::MissingSection(SectionType::Data))?;
        let (data_offset, data_len) = (data_entry.offset, data_entry.length);

        Ok(Self {
            file,
            superblock,
            sections,
            tracks,
            dict,
            chunks,
            segments,
            meta,
            integrity,
            data_offset,
            data_len,
        })
    }

    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }
    pub fn sections(&self) -> &[SectionEntry] {
        &self.sections
    }
    pub fn tracks(&self) -> &[TrackRecord] {
        &self.tracks
    }
    pub fn dict(&self) -> &[u32] {
        &self.dict
    }
    pub fn chunks(&self) -> &[ChunkRecord] {
        &self.chunks
    }
    pub fn segments(&self) -> &[SegmentRecord] {
        &self.segments
    }
    pub fn meta(&self) -> &[(String, String)] {
        &self.meta
    }
    pub fn integrity(&self) -> &Integrity {
        &self.integrity
    }

    pub fn track(&self, track_id: u32) -> Result<&TrackRecord> {
        self.tracks
            .iter()
            .find(|t| t.track_id == track_id)
            .ok_or(FormatError::TrackNotFound(track_id))
    }

    /// Segments of one track, ordered by presentation time.
    pub fn segments_for_track(&self, track_id: u32) -> Vec<&SegmentRecord> {
        let mut segs: Vec<&SegmentRecord> = self
            .segments
            .iter()
            .filter(|s| s.track_id == track_id)
            .collect();
        segs.sort_by_key(|s| (s.pts_start, s.segment_id));
        segs
    }

    /// Read one chunk payload exactly as stored (possibly zstd-compressed),
    /// without decompressing or verifying. Returns (stored bytes, flags,
    /// len_raw). Intended for wire passthrough: the receiver decompresses
    /// and verifies the BLAKE3 identity against the raw bytes.
    pub fn read_chunk_stored(&mut self, index: u32) -> Result<(Vec<u8>, u32, u32)> {
        let rec = self
            .chunks
            .get(index as usize)
            .ok_or(FormatError::ChunkIndexOutOfRange(index))?
            .clone();
        // The chunk's stored bytes must lie fully within the DATA section.
        if rec.data_offset > self.data_len
            || rec.len_stored as u64 > self.data_len - rec.data_offset
            || rec.len_raw as u64 > crate::MAX_CHUNK_RAW
        {
            return Err(FormatError::Malformed("chunk bounds"));
        }
        self.file
            .seek(SeekFrom::Start(self.data_offset + rec.data_offset))?;
        let mut stored = vec![0u8; rec.len_stored as usize];
        self.file.read_exact(&mut stored)?;
        Ok((stored, rec.flags, rec.len_raw))
    }

    /// Read, decompress and verify one chunk payload.
    pub fn read_chunk(&mut self, index: u32) -> Result<Vec<u8>> {
        let (stored, flags, len_raw) = self.read_chunk_stored(index)?;
        let raw = if flags & CHUNK_FLAG_ZSTD != 0 {
            // len_raw was bounded by MAX_CHUNK_RAW in read_chunk_stored, so
            // the decompression capacity hint is safe.
            zstd::bulk::decompress(&stored, len_raw as usize).map_err(FormatError::Zstd)?
        } else {
            stored
        };
        let rec = &self.chunks[index as usize];
        if raw.len() != len_raw as usize || hash_chunk(&raw) != rec.hash {
            return Err(FormatError::ChunkHashMismatch { index });
        }
        Ok(raw)
    }

    /// Reconstruct a segment payload: ordered concatenation of its chunks.
    pub fn segment_bytes(&mut self, segment: &SegmentRecord) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for &c in &segment.chunks {
            out.extend_from_slice(&self.read_chunk(c)?);
        }
        Ok(out)
    }

    /// Reconstruct a track's init payload (e.g. CMAF init segment).
    pub fn track_init_bytes(&mut self, track_id: u32) -> Result<Vec<u8>> {
        let init_chunks = self.track(track_id)?.init_chunks.clone();
        let mut out = Vec::new();
        for c in init_chunks {
            out.extend_from_slice(&self.read_chunk(c)?);
        }
        Ok(out)
    }

    /// Embedded content signature (sig, pubkey) if present, parsed from meta.
    pub fn embedded_signature(&self) -> Option<([u8; 64], [u8; 32])> {
        let hex_bytes = |key: &str, len: usize| -> Option<Vec<u8>> {
            let value = self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v)?;
            if value.len() != len * 2 {
                return None;
            }
            (0..len)
                .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok())
                .collect()
        };
        let sig: [u8; 64] = hex_bytes("sig.ed25519", 64)?.try_into().ok()?;
        let pk: [u8; 32] = hex_bytes("sig.pubkey", 32)?.try_into().ok()?;
        Some((sig, pk))
    }

    /// Check the embedded Ed25519 content signature, if any. Returns
    /// `Unsigned` when absent, `Valid(pubkey)` when it verifies, and an error
    /// when present but invalid. Callers decide whether the returned pubkey
    /// is trusted.
    pub fn verify_signature(&self) -> Result<SignatureStatus> {
        let Some((sig, pk)) = self.embedded_signature() else {
            return Ok(SignatureStatus::Unsigned);
        };
        let key = ed25519_dalek::VerifyingKey::from_bytes(&pk)
            .map_err(|_| FormatError::SignatureInvalid)?;
        let message =
            content_signature_message(&self.integrity.merkle_root, self.integrity.chunk_count);
        use ed25519_dalek::Verifier;
        key.verify(&message, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| FormatError::SignatureInvalid)?;
        Ok(SignatureStatus::Valid(pk))
    }

    /// Full verification: every chunk hash, the Merkle root against the
    /// integrity section, and the DATA section hash from the directory.
    pub fn verify(&mut self) -> Result<VerifyReport> {
        let mut bytes = 0u64;
        for i in 0..self.chunks.len() as u32 {
            bytes += self.read_chunk(i)?.len() as u64;
        }

        let hashes: Vec<_> = self.chunks.iter().map(|c| c.hash).collect();
        if merkle_root(&hashes) != self.integrity.merkle_root
            || self.integrity.chunk_count != self.chunks.len() as u64
        {
            return Err(FormatError::MerkleMismatch);
        }

        // Stream-hash the DATA section against its directory entry.
        let data_entry_hash = self
            .sections
            .iter()
            .find(|s| s.section_type == SectionType::Data)
            .map(|s| s.hash)
            .ok_or(FormatError::MissingSection(SectionType::Data))?;
        self.file.seek(SeekFrom::Start(self.data_offset))?;
        let mut hasher = Hasher::new();
        let mut remaining = self.data_len;
        let mut buf = vec![0u8; 1 << 20];
        while remaining > 0 {
            let n = remaining.min(buf.len() as u64) as usize;
            self.file.read_exact(&mut buf[..n])?;
            hasher.update(&buf[..n]);
            remaining -= n as u64;
        }
        if hasher.finalize() != data_entry_hash {
            return Err(FormatError::SectionHashMismatch(SectionType::Data));
        }

        Ok(VerifyReport {
            chunks_verified: self.chunks.len() as u64,
            bytes_verified: bytes,
            merkle_ok: true,
            data_section_ok: true,
        })
    }
}

fn decode_tracks(buf: &[u8]) -> Result<Vec<TrackRecord>> {
    let mut cur = Cursor::new(buf, "tracks");
    let count = cur.u32()?;
    let mut out = Vec::with_capacity((count as usize).min(buf.len()));
    for _ in 0..count {
        let track_id = cur.u32()?;
        let kind_raw = cur.u8()?;
        let kind = TrackKind::from_u8(kind_raw).ok_or(FormatError::UnknownValue {
            what: "track kind",
            value: kind_raw as u32,
        })?;
        let flags = cur.u8()?;
        let codec = cur.str16()?;
        let name = cur.str16()?;
        let timescale = cur.u32()?;
        let n = cur.u32()?;
        let mut init_chunks = Vec::with_capacity((n as usize).min(buf.len()));
        for _ in 0..n {
            init_chunks.push(cur.u32()?);
        }
        out.push(TrackRecord {
            track_id,
            kind,
            flags,
            codec,
            name,
            timescale,
            init_chunks,
        });
    }
    Ok(out)
}

fn decode_dict(buf: &[u8]) -> Result<Vec<u32>> {
    let mut cur = Cursor::new(buf, "dict");
    let count = cur.u32()?;
    let mut out = Vec::with_capacity((count as usize).min(buf.len()));
    for _ in 0..count {
        out.push(cur.u32()?);
    }
    Ok(out)
}

fn decode_chunks(buf: &[u8]) -> Result<Vec<ChunkRecord>> {
    let mut cur = Cursor::new(buf, "chunks");
    let count = cur.u32()?;
    let mut out = Vec::with_capacity((count as usize).min(buf.len()));
    for _ in 0..count {
        out.push(ChunkRecord {
            hash: cur.hash()?,
            data_offset: cur.u64()?,
            len_raw: cur.u32()?,
            len_stored: cur.u32()?,
            flags: cur.u32()?,
        });
    }
    Ok(out)
}

fn decode_segments(buf: &[u8]) -> Result<Vec<SegmentRecord>> {
    let mut cur = Cursor::new(buf, "segments");
    let count = cur.u32()?;
    let mut out = Vec::with_capacity((count as usize).min(buf.len()));
    for _ in 0..count {
        let segment_id = cur.u64()?;
        let track_id = cur.u32()?;
        let pts_start = cur.u64()?;
        let duration = cur.u32()?;
        let flags = cur.u32()?;
        let n = cur.u32()?;
        let mut chunks = Vec::with_capacity((n as usize).min(buf.len()));
        for _ in 0..n {
            chunks.push(cur.u32()?);
        }
        out.push(SegmentRecord {
            segment_id,
            track_id,
            pts_start,
            duration,
            flags,
            chunks,
        });
    }
    Ok(out)
}

fn decode_meta(buf: &[u8]) -> Result<Vec<(String, String)>> {
    let mut cur = Cursor::new(buf, "meta");
    let count = cur.u32()?;
    let mut out = Vec::with_capacity((count as usize).min(buf.len()));
    for _ in 0..count {
        let key = cur.str16()?;
        let value_bytes = cur.bytes32()?;
        let value = String::from_utf8(value_bytes).map_err(|_| FormatError::Malformed("meta"))?;
        out.push((key, value));
    }
    Ok(out)
}

fn decode_integrity(buf: &[u8]) -> Result<Integrity> {
    let mut cur = Cursor::new(buf, "integrity");
    Ok(Integrity {
        merkle_root: cur.hash()?,
        chunk_count: cur.u64()?,
        total_raw: cur.u64()?,
        total_stored: cur.u64()?,
    })
}
