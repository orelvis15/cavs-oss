//! Streaming writer for CAVS-1 files.
//!
//! Chunk payloads are deduplicated at ingest and streamed straight into the
//! DATA section as they arrive; only the (small) tables are buffered until
//! `finish()`.

use crate::wire::*;
use crate::{
    ChunkRecord, FormatError, Integrity, Result, SectionType, SegmentRecord, TrackRecord,
    CHUNK_FLAG_ZSTD, COMPRESSION_NONE, COMPRESSION_ZSTD, MAGIC, SUPERBLOCK_LEN, VERSION_MAJOR,
    VERSION_MINOR,
};
use cavs_hash::{content_signature_message, hash_chunk, merkle_root, ChunkHash, Hasher};
use cavs_store::CasIndex;
use ed25519_dalek::{Signer, SigningKey};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write as _};
use std::path::Path;

/// Minimum chunk size worth attempting compression on.
const COMPRESS_MIN_LEN: usize = 512;

/// Summary of a finished pack, for reporting.
#[derive(Debug, Clone)]
pub struct PackStats {
    pub file_size: u64,
    pub unique_chunks: u64,
    pub logical_chunks: u64,
    /// Bytes before dedup (every add_chunk call counted).
    pub logical_raw: u64,
    /// Bytes of unique chunks, uncompressed.
    pub unique_raw: u64,
    /// Bytes of unique chunks as stored (after compression).
    pub stored: u64,
    pub merkle_root: ChunkHash,
}

pub struct Writer {
    out: BufWriter<File>,
    cas: CasIndex,
    chunks: Vec<ChunkRecord>,
    tracks: Vec<TrackRecord>,
    segments: Vec<SegmentRecord>,
    dict: Vec<u32>,
    meta: Vec<(String, String)>,
    data_len: u64,
    data_hasher: Hasher,
    logical_raw: u64,
    logical_chunks: u64,
    compression: u8,
    zstd_level: i32,
    timescale: u32,
    asset_uuid: [u8; 16],
    signer: Option<SigningKey>,
}

impl Writer {
    /// Create a new CAVS-1 file at `path`. `compress` enables per-chunk zstd
    /// (chunks that don't shrink are stored raw regardless).
    pub fn create(
        path: &Path,
        asset_uuid: [u8; 16],
        timescale: u32,
        compress: bool,
    ) -> Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        // Superblock placeholder; patched in finish().
        out.write_all(&[0u8; SUPERBLOCK_LEN as usize])?;
        Ok(Self {
            out,
            cas: CasIndex::new(),
            chunks: Vec::new(),
            tracks: Vec::new(),
            segments: Vec::new(),
            dict: Vec::new(),
            meta: Vec::new(),
            data_len: 0,
            data_hasher: Hasher::new(),
            logical_raw: 0,
            logical_chunks: 0,
            compression: if compress {
                COMPRESSION_ZSTD
            } else {
                COMPRESSION_NONE
            },
            zstd_level: 3,
            timescale,
            asset_uuid,
            signer: None,
        })
    }

    /// zstd level for chunk storage/wire compression (default 3).
    pub fn set_zstd_level(&mut self, level: i32) {
        self.zstd_level = level;
    }

    /// Sign the packed content with this Ed25519 secret key. The signature
    /// (over the canonical content message, see
    /// [`cavs_hash::content_signature_message`]) and the public key are
    /// embedded as `sig.ed25519` / `sig.pubkey` meta entries at finish().
    pub fn sign_with(&mut self, secret: &[u8; 32]) {
        self.signer = Some(SigningKey::from_bytes(secret));
    }

    /// Add one chunk payload. Returns its chunk-table index. Duplicate
    /// payloads (same BLAKE3) are not stored again.
    pub fn add_chunk(&mut self, raw: &[u8]) -> Result<u32> {
        self.logical_raw += raw.len() as u64;
        self.logical_chunks += 1;
        let hash = hash_chunk(raw);
        let interned = self.cas.intern(hash, self.chunks.len() as u32);
        if !interned.is_new() {
            return Ok(interned.index());
        }

        let mut flags = 0u32;
        let stored: Vec<u8>;
        let stored_slice: &[u8] =
            if self.compression == COMPRESSION_ZSTD && raw.len() >= COMPRESS_MIN_LEN {
                stored = zstd::bulk::compress(raw, self.zstd_level).map_err(FormatError::Zstd)?;
                // Keep compression only if it actually pays for itself.
                if stored.len() < raw.len() - raw.len() / 16 {
                    flags |= CHUNK_FLAG_ZSTD;
                    &stored
                } else {
                    raw
                }
            } else {
                raw
            };

        self.out.write_all(stored_slice)?;
        self.data_hasher.update(stored_slice);
        self.chunks.push(ChunkRecord {
            hash,
            data_offset: self.data_len,
            len_raw: raw.len() as u32,
            len_stored: stored_slice.len() as u32,
            flags,
        });
        self.data_len += stored_slice.len() as u64;
        Ok(interned.index())
    }

    pub fn add_track(&mut self, track: TrackRecord) -> Result<()> {
        for &c in &track.init_chunks {
            self.check_chunk_index(c)?;
        }
        self.tracks.push(track);
        Ok(())
    }

    pub fn add_segment(&mut self, segment: SegmentRecord) -> Result<()> {
        for &c in &segment.chunks {
            self.check_chunk_index(c)?;
        }
        self.segments.push(segment);
        Ok(())
    }

    /// Mark a chunk as part of the global dictionary (privileged reuse:
    /// init segments, bootstrap assets, shared headers...).
    pub fn pin_dict(&mut self, chunk_index: u32) -> Result<()> {
        self.check_chunk_index(chunk_index)?;
        if !self.dict.contains(&chunk_index) {
            self.dict.push(chunk_index);
        }
        Ok(())
    }

    pub fn set_meta(&mut self, key: &str, value: &str) {
        self.meta.push((key.to_string(), value.to_string()));
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunks.len() as u32
    }

    fn check_chunk_index(&self, index: u32) -> Result<()> {
        if (index as usize) < self.chunks.len() {
            Ok(())
        } else {
            Err(FormatError::ChunkIndexOutOfRange(index))
        }
    }

    /// Write all tables, the section directory and the final superblock.
    pub fn finish(mut self) -> Result<PackStats> {
        let hashes: Vec<ChunkHash> = self.chunks.iter().map(|c| c.hash).collect();
        let root = merkle_root(&hashes);
        let unique_raw: u64 = self.chunks.iter().map(|c| c.len_raw as u64).sum();
        let stored: u64 = self.chunks.iter().map(|c| c.len_stored as u64).sum();

        let integrity = Integrity {
            merkle_root: root,
            chunk_count: self.chunks.len() as u64,
            total_raw: unique_raw,
            total_stored: stored,
        };

        if let Some(signer) = &self.signer {
            let message = content_signature_message(&root, integrity.chunk_count);
            let sig = signer.sign(&message);
            let sig_hex: String = sig.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
            let pk_hex: String = signer
                .verifying_key()
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            self.meta.push(("sig.ed25519".to_string(), sig_hex));
            self.meta.push(("sig.pubkey".to_string(), pk_hex));
        }

        // DATA was streamed right after the superblock.
        let mut dir: Vec<(SectionType, u64, u64, ChunkHash)> = vec![(
            SectionType::Data,
            SUPERBLOCK_LEN,
            self.data_len,
            self.data_hasher.finalize(),
        )];

        let sections: Vec<(SectionType, Vec<u8>)> = vec![
            (SectionType::Tracks, encode_tracks(&self.tracks)),
            (SectionType::Dict, encode_dict(&self.dict)),
            (SectionType::Chunks, encode_chunks(&self.chunks)),
            (SectionType::Segments, encode_segments(&self.segments)),
            (SectionType::Meta, encode_meta(&self.meta)),
            (SectionType::Integrity, encode_integrity(&integrity)),
        ];

        let mut offset = SUPERBLOCK_LEN + self.data_len;
        for (ty, bytes) in &sections {
            self.out.write_all(bytes)?;
            dir.push((*ty, offset, bytes.len() as u64, hash_chunk(bytes)));
            offset += bytes.len() as u64;
        }

        let section_dir_offset = offset;
        let mut dir_bytes = Vec::with_capacity(dir.len() * crate::SECTION_DIR_ENTRY_LEN);
        for (ty, off, len, hash) in &dir {
            put_u32(&mut dir_bytes, *ty as u32);
            put_u64(&mut dir_bytes, *off);
            put_u64(&mut dir_bytes, *len);
            dir_bytes.extend_from_slice(hash);
        }
        self.out.write_all(&dir_bytes)?;
        let file_size = section_dir_offset + dir_bytes.len() as u64;

        // Patch the superblock.
        let mut sb = Vec::with_capacity(SUPERBLOCK_LEN as usize);
        sb.extend_from_slice(&MAGIC);
        put_u16(&mut sb, VERSION_MAJOR);
        put_u16(&mut sb, VERSION_MINOR);
        put_u32(&mut sb, 0); // feature flags
        sb.push(cavs_hash::HashAlgo::Blake3 as u8);
        sb.push(self.compression);
        put_u16(&mut sb, 0); // reserved
        sb.extend_from_slice(&self.asset_uuid);
        put_u32(&mut sb, self.timescale);
        put_u32(&mut sb, dir.len() as u32);
        put_u64(&mut sb, section_dir_offset);
        put_u64(&mut sb, file_size);
        sb.resize(SUPERBLOCK_LEN as usize, 0);

        self.out.flush()?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&sb)?;
        file.sync_all()?;

        Ok(PackStats {
            file_size,
            unique_chunks: self.chunks.len() as u64,
            logical_chunks: self.logical_chunks,
            logical_raw: self.logical_raw,
            unique_raw,
            stored,
            merkle_root: root,
        })
    }
}

fn encode_tracks(tracks: &[TrackRecord]) -> Vec<u8> {
    let mut buf = Vec::new();
    put_u32(&mut buf, tracks.len() as u32);
    for t in tracks {
        put_u32(&mut buf, t.track_id);
        buf.push(t.kind as u8);
        buf.push(t.flags);
        put_str(&mut buf, &t.codec);
        put_str(&mut buf, &t.name);
        put_u32(&mut buf, t.timescale);
        put_u32(&mut buf, t.init_chunks.len() as u32);
        for &c in &t.init_chunks {
            put_u32(&mut buf, c);
        }
    }
    buf
}

fn encode_dict(dict: &[u32]) -> Vec<u8> {
    let mut buf = Vec::new();
    put_u32(&mut buf, dict.len() as u32);
    for &c in dict {
        put_u32(&mut buf, c);
    }
    buf
}

fn encode_chunks(chunks: &[ChunkRecord]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + chunks.len() * 52);
    put_u32(&mut buf, chunks.len() as u32);
    for c in chunks {
        buf.extend_from_slice(&c.hash);
        put_u64(&mut buf, c.data_offset);
        put_u32(&mut buf, c.len_raw);
        put_u32(&mut buf, c.len_stored);
        put_u32(&mut buf, c.flags);
    }
    buf
}

fn encode_segments(segments: &[SegmentRecord]) -> Vec<u8> {
    let mut buf = Vec::new();
    put_u32(&mut buf, segments.len() as u32);
    for s in segments {
        put_u64(&mut buf, s.segment_id);
        put_u32(&mut buf, s.track_id);
        put_u64(&mut buf, s.pts_start);
        put_u32(&mut buf, s.duration);
        put_u32(&mut buf, s.flags);
        put_u32(&mut buf, s.chunks.len() as u32);
        for &c in &s.chunks {
            put_u32(&mut buf, c);
        }
    }
    buf
}

fn encode_meta(meta: &[(String, String)]) -> Vec<u8> {
    let mut buf = Vec::new();
    put_u32(&mut buf, meta.len() as u32);
    for (k, v) in meta {
        put_str(&mut buf, k);
        put_bytes32(&mut buf, v.as_bytes());
    }
    buf
}

fn encode_integrity(i: &Integrity) -> Vec<u8> {
    let mut buf = Vec::with_capacity(56);
    buf.extend_from_slice(&i.merkle_root);
    put_u64(&mut buf, i.chunk_count);
    put_u64(&mut buf, i.total_raw);
    put_u64(&mut buf, i.total_stored);
    buf
}
