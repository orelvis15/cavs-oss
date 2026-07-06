//! CAVS Signature v1 (`.cavssig`): a compact description of an old
//! artifact or directory tree — layout, sizes and per-block hashes — so a
//! new version can be compared against it without the old content.
//!
//! CAVS-native design: BLAKE3-256 is the only identity/verification hash;
//! the weak 32-bit rolling hash is a prefilter and never trusted on its own.
//!
//! Wire layout (all multi-byte integers are LEB128 varints unless noted):
//!
//! ```text
//! [8]  magic  "CAVSSIG1"
//! u16  version = 1                     (LE, fixed width)
//! u8   kind                            (1 artifact, 2 directory)
//! var  created_at_unix_ms              (0 for deterministic output)
//! var  block_size
//! var  source_size
//! u8   has_source_blake3; [32] if set
//! str  source_label                    (var len + UTF-8 bytes)
//! str  chunker_profile
//! var  entry_count
//!      entry_count × { var entry_id; str path; u8 kind; var size;
//!                      u8 executable; str symlink_target ("" = none) }
//! var  block_count
//!      block_count × { var entry_id; var offset; var len;
//!                      u32 weak32 (LE); [32] strong_blake3 }
//! [32] merkle_root                     (over block strong hashes, in order)
//! [32] BLAKE3 of every preceding byte  (integrity trailer)
//! ```
//!
//! Blocks of one entry must be contiguous from offset 0 and cover the
//! entry's size exactly; the decoder enforces it, so a decoded signature is
//! always internally consistent. Encoding is deterministic: the same
//! logical signature always produces the same bytes.

pub mod diff;
pub mod weak;

use cavs_hash::{hash_chunk, merkle_root, ChunkHash, Hasher};
use std::io::Read;
use std::path::Path;

pub const SIGNATURE_MAGIC: [u8; 8] = *b"CAVSSIG1";
pub const SIGNATURE_VERSION: u16 = 1;
/// Default block size: 64 KiB (the empirically chosen sweet spot for
/// block-based delta scanning — larger blocks shrink the signature but
/// grow the patch, since small edits incur a full-block penalty).
pub const DEFAULT_BLOCK_SIZE: u32 = 64 * 1024;
/// Sanity caps: reject hostile counts before allocating.
pub const MAX_ENTRIES: u64 = 1 << 24;
pub const MAX_BLOCKS: u64 = 1 << 28;
/// Block sizes outside this range are malformed.
pub const MIN_BLOCK_SIZE: u32 = 1 << 10;
pub const MAX_BLOCK_SIZE: u32 = 1 << 26;

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("bad .cavssig magic")]
    BadMagic,
    #[error("unsupported .cavssig version {0}")]
    UnsupportedVersion(u16),
    #[error("truncated .cavssig: {0}")]
    Truncated(&'static str),
    #[error("malformed .cavssig: {0}")]
    Malformed(&'static str),
    #[error(".cavssig integrity trailer mismatch (file corrupt)")]
    IntegrityMismatch,
    #[error("signature does not match source: {0}")]
    SourceMismatch(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, SignatureError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureKind {
    SingleArtifact = 1,
    DirectoryContainer = 2,
}

impl SignatureKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(SignatureKind::SingleArtifact),
            2 => Some(SignatureKind::DirectoryContainer),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SignatureKind::SingleArtifact => "single-artifact",
            SignatureKind::DirectoryContainer => "directory-container",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryKind {
    File = 1,
    Directory = 2,
    Symlink = 3,
}

impl EntryKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(EntryKind::File),
            2 => Some(EntryKind::Directory),
            3 => Some(EntryKind::Symlink),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureEntry {
    pub entry_id: u32,
    /// Relative path inside the container; the artifact name (or "") for
    /// single-artifact signatures.
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub executable: bool,
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureBlockHash {
    pub entry_id: u32,
    pub offset: u64,
    pub len: u32,
    pub weak32: u32,
    pub strong_blake3: ChunkHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CavsSignature {
    pub kind: SignatureKind,
    /// 0 in deterministic exports (the default): same input, same bytes.
    pub created_at_unix_ms: u64,
    pub source_label: String,
    pub source_size: u64,
    /// Full BLAKE3 of the source content (file bytes, or every file's bytes
    /// in entry order for directories).
    pub source_blake3: Option<ChunkHash>,
    pub chunker_profile: String,
    pub block_size: u32,
    pub entries: Vec<SignatureEntry>,
    pub blocks: Vec<SignatureBlockHash>,
    pub merkle_root: ChunkHash,
}

// ---------------------------------------------------------------------------
// Varint helpers (strict LEB128, same rules as the binary manifest)
// ---------------------------------------------------------------------------

fn write_var(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_var(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for i in 0..10 {
        let Some(&byte) = input.get(i) else {
            return Err(SignatureError::Truncated("varint"));
        };
        if byte == 0 && shift != 0 {
            return Err(SignatureError::Malformed("overlong varint"));
        }
        if i == 9 && byte > 1 {
            return Err(SignatureError::Malformed("varint overflow"));
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            *input = &input[i + 1..];
            return Ok(value);
        }
        shift += 7;
    }
    Err(SignatureError::Malformed("overlong varint"))
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    write_var(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

fn read_str(input: &mut &[u8]) -> Result<String> {
    let len = read_var(input)? as usize;
    if len > input.len() || len > 1 << 16 {
        return Err(SignatureError::Truncated("string"));
    }
    let (head, tail) = input.split_at(len);
    *input = tail;
    String::from_utf8(head.to_vec()).map_err(|_| SignatureError::Malformed("string not UTF-8"))
}

fn take<'a>(input: &mut &'a [u8], n: usize, what: &'static str) -> Result<&'a [u8]> {
    if n > input.len() {
        return Err(SignatureError::Truncated(what));
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

// ---------------------------------------------------------------------------
// Encode / decode
// ---------------------------------------------------------------------------

impl CavsSignature {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.entries.len() * 32 + self.blocks.len() * 48);
        out.extend_from_slice(&SIGNATURE_MAGIC);
        out.extend_from_slice(&SIGNATURE_VERSION.to_le_bytes());
        out.push(self.kind as u8);
        write_var(self.created_at_unix_ms, &mut out);
        write_var(self.block_size as u64, &mut out);
        write_var(self.source_size, &mut out);
        match &self.source_blake3 {
            Some(h) => {
                out.push(1);
                out.extend_from_slice(h);
            }
            None => out.push(0),
        }
        write_str(&self.source_label, &mut out);
        write_str(&self.chunker_profile, &mut out);
        write_var(self.entries.len() as u64, &mut out);
        for e in &self.entries {
            write_var(e.entry_id as u64, &mut out);
            write_str(&e.path, &mut out);
            out.push(e.kind as u8);
            write_var(e.size, &mut out);
            out.push(e.executable as u8);
            write_str(e.symlink_target.as_deref().unwrap_or(""), &mut out);
        }
        write_var(self.blocks.len() as u64, &mut out);
        for b in &self.blocks {
            write_var(b.entry_id as u64, &mut out);
            write_var(b.offset, &mut out);
            write_var(b.len as u64, &mut out);
            out.extend_from_slice(&b.weak32.to_le_bytes());
            out.extend_from_slice(&b.strong_blake3);
        }
        out.extend_from_slice(&self.merkle_root);
        let trailer = hash_chunk(&out);
        out.extend_from_slice(&trailer);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SIGNATURE_MAGIC.len() + 2 + 32 {
            return Err(SignatureError::Truncated("header"));
        }
        if bytes[..8] != SIGNATURE_MAGIC {
            return Err(SignatureError::BadMagic);
        }
        // Integrity trailer first: everything else assumes intact bytes.
        let body_len = bytes.len() - 32;
        let expected: ChunkHash = bytes[body_len..].try_into().unwrap();
        if hash_chunk(&bytes[..body_len]) != expected {
            return Err(SignatureError::IntegrityMismatch);
        }

        let mut input = &bytes[8..body_len];
        let version = u16::from_le_bytes(take(&mut input, 2, "version")?.try_into().unwrap());
        if version != SIGNATURE_VERSION {
            return Err(SignatureError::UnsupportedVersion(version));
        }
        let kind = SignatureKind::from_u8(take(&mut input, 1, "kind")?[0])
            .ok_or(SignatureError::Malformed("unknown signature kind"))?;
        let created_at_unix_ms = read_var(&mut input)?;
        let block_size = read_var(&mut input)?;
        if !(MIN_BLOCK_SIZE as u64..=MAX_BLOCK_SIZE as u64).contains(&block_size) {
            return Err(SignatureError::Malformed("block size out of range"));
        }
        let block_size = block_size as u32;
        let source_size = read_var(&mut input)?;
        let source_blake3 = match take(&mut input, 1, "source hash flag")?[0] {
            0 => None,
            1 => Some(take(&mut input, 32, "source hash")?.try_into().unwrap()),
            _ => return Err(SignatureError::Malformed("source hash flag")),
        };
        let source_label = read_str(&mut input)?;
        let chunker_profile = read_str(&mut input)?;

        let entry_count = read_var(&mut input)?;
        if entry_count > MAX_ENTRIES {
            return Err(SignatureError::Malformed("entry count too large"));
        }
        let mut entries = Vec::with_capacity((entry_count as usize).min(input.len() / 4));
        let mut seen_ids = std::collections::HashSet::new();
        for _ in 0..entry_count {
            let entry_id = read_var(&mut input)?;
            if entry_id > u32::MAX as u64 || !seen_ids.insert(entry_id as u32) {
                return Err(SignatureError::Malformed("duplicate or invalid entry id"));
            }
            let path = read_str(&mut input)?;
            let kind = EntryKind::from_u8(take(&mut input, 1, "entry kind")?[0])
                .ok_or(SignatureError::Malformed("unknown entry kind"))?;
            let size = read_var(&mut input)?;
            let executable = match take(&mut input, 1, "executable flag")?[0] {
                0 => false,
                1 => true,
                _ => return Err(SignatureError::Malformed("executable flag")),
            };
            let target = read_str(&mut input)?;
            entries.push(SignatureEntry {
                entry_id: entry_id as u32,
                path,
                kind,
                size,
                executable,
                symlink_target: (!target.is_empty()).then_some(target),
            });
        }

        let block_count = read_var(&mut input)?;
        if block_count > MAX_BLOCKS {
            return Err(SignatureError::Malformed("block count too large"));
        }
        let mut blocks = Vec::with_capacity((block_count as usize).min(input.len() / 40));
        // Per-entry coverage: blocks must be contiguous from 0 and end at
        // the entry's size, so ranges derived from a signature are always
        // valid reads of the source.
        let mut covered: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        for _ in 0..block_count {
            let entry_id = read_var(&mut input)?;
            if entry_id > u32::MAX as u64 || !seen_ids.contains(&(entry_id as u32)) {
                return Err(SignatureError::Malformed("block references unknown entry"));
            }
            let entry_id = entry_id as u32;
            let offset = read_var(&mut input)?;
            let len = read_var(&mut input)?;
            if len == 0 || len > block_size as u64 {
                return Err(SignatureError::Malformed("block length out of range"));
            }
            let expected_offset = covered.get(&entry_id).copied().unwrap_or(0);
            if offset != expected_offset {
                return Err(SignatureError::Malformed("blocks not contiguous"));
            }
            covered.insert(entry_id, offset + len);
            let weak = u32::from_le_bytes(take(&mut input, 4, "weak hash")?.try_into().unwrap());
            let strong: ChunkHash = take(&mut input, 32, "strong hash")?.try_into().unwrap();
            blocks.push(SignatureBlockHash {
                entry_id,
                offset,
                len: len as u32,
                weak32: weak,
                strong_blake3: strong,
            });
        }
        for e in &entries {
            let got = covered.get(&e.entry_id).copied().unwrap_or(0);
            let want = if e.kind == EntryKind::File { e.size } else { 0 };
            if got != want {
                return Err(SignatureError::Malformed("blocks do not cover entry size"));
            }
        }

        let root: ChunkHash = take(&mut input, 32, "merkle root")?.try_into().unwrap();
        if !input.is_empty() {
            return Err(SignatureError::Malformed("trailing bytes"));
        }
        let leaves: Vec<ChunkHash> = blocks.iter().map(|b| b.strong_blake3).collect();
        if merkle_root(&leaves) != root {
            return Err(SignatureError::Malformed("merkle root mismatch"));
        }

        Ok(CavsSignature {
            kind,
            created_at_unix_ms,
            source_label,
            source_size,
            source_blake3,
            chunker_profile,
            block_size,
            entries,
            blocks,
            merkle_root: root,
        })
    }

    /// Sign one file (streaming; peak RAM = one block).
    pub fn sign_file(path: &Path, block_size: u32, label: &str) -> Result<Self> {
        let mut b = SignatureBuilder::new(SignatureKind::SingleArtifact, block_size, "fixed");
        b.begin_entry(label, false, None);
        let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            b.append(&buf[..n]);
        }
        Ok(b.finish(label))
    }

    /// Sign a directory tree (files walked in sorted order; symlinks are
    /// recorded, not followed).
    pub fn sign_dir(root: &Path, block_size: u32, label: &str) -> Result<Self> {
        let mut b = SignatureBuilder::new(SignatureKind::DirectoryContainer, block_size, "fixed");
        let mut paths = walk_sorted(root)?;
        paths.sort();
        let mut buf = vec![0u8; 1 << 20];
        for rel in paths {
            let full = root.join(&rel);
            let meta = std::fs::symlink_metadata(&full)?;
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&full)?;
                b.begin_entry(&rel_str, false, Some(&target.to_string_lossy()));
                b.entry_kind(EntryKind::Symlink);
            } else if meta.is_dir() {
                b.begin_entry(&rel_str, false, None);
                b.entry_kind(EntryKind::Directory);
            } else {
                b.begin_entry(&rel_str, is_executable(&meta), None);
                let mut file = std::io::BufReader::new(std::fs::File::open(&full)?);
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    b.append(&buf[..n]);
                }
            }
        }
        Ok(b.finish(label))
    }

    /// Verify this signature against a source file or directory: recompute
    /// every block hash and compare. Cheap relative to a download; O(source).
    pub fn verify_against(&self, source: &Path) -> Result<()> {
        let fresh = match self.kind {
            SignatureKind::SingleArtifact => {
                CavsSignature::sign_file(source, self.block_size, &self.source_label)?
            }
            SignatureKind::DirectoryContainer => {
                CavsSignature::sign_dir(source, self.block_size, &self.source_label)?
            }
        };
        if fresh.source_size != self.source_size {
            return Err(SignatureError::SourceMismatch(format!(
                "size differs: signature says {} bytes, source has {}",
                self.source_size, fresh.source_size
            )));
        }
        if let (Some(a), Some(b)) = (&self.source_blake3, &fresh.source_blake3) {
            if a != b {
                return Err(SignatureError::SourceMismatch(
                    "content BLAKE3 differs".to_string(),
                ));
            }
        }
        if fresh.merkle_root != self.merkle_root {
            let mismatched = self
                .blocks
                .iter()
                .zip(fresh.blocks.iter())
                .filter(|(a, b)| a.strong_blake3 != b.strong_blake3)
                .count()
                .max(self.blocks.len().abs_diff(fresh.blocks.len()));
            return Err(SignatureError::SourceMismatch(format!(
                "{mismatched} block(s) differ"
            )));
        }
        Ok(())
    }
}

fn walk_sorted(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(|e| e.path())
            .collect();
        children.sort();
        for child in children {
            let rel = child.strip_prefix(root).unwrap().to_path_buf();
            let meta = std::fs::symlink_metadata(&child)?;
            out.push(rel);
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(child);
            }
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Streaming builder
// ---------------------------------------------------------------------------

/// Builds a signature by streaming bytes through a fixed-size block buffer:
/// callers `begin_entry`, `append` any number of times, and `finish`. Peak
/// memory is one block regardless of source size. Used by file/dir signing
/// and by `.cavs` exports (which stream decoded chunks through it).
pub struct SignatureBuilder {
    kind: SignatureKind,
    block_size: u32,
    profile: String,
    entries: Vec<SignatureEntry>,
    blocks: Vec<SignatureBlockHash>,
    content_hasher: Hasher,
    total_size: u64,
    buf: Vec<u8>,
    current_offset: u64,
}

impl SignatureBuilder {
    pub fn new(kind: SignatureKind, block_size: u32, profile: &str) -> Self {
        let block_size = block_size.clamp(MIN_BLOCK_SIZE, MAX_BLOCK_SIZE);
        Self {
            kind,
            block_size,
            profile: profile.to_string(),
            entries: Vec::new(),
            blocks: Vec::new(),
            content_hasher: Hasher::new(),
            total_size: 0,
            buf: Vec::with_capacity(block_size as usize),
            current_offset: 0,
        }
    }

    /// Start a new entry (defaults to a File; see [`Self::entry_kind`]).
    /// Flushes any pending short block of the previous entry.
    pub fn begin_entry(&mut self, path: &str, executable: bool, symlink_target: Option<&str>) {
        self.flush_block();
        let entry_id = self.entries.len() as u32;
        self.entries.push(SignatureEntry {
            entry_id,
            path: path.to_string(),
            kind: EntryKind::File,
            size: 0,
            executable,
            symlink_target: symlink_target.map(str::to_string),
        });
        self.current_offset = 0;
    }

    /// Reclassify the current entry (directories and symlinks carry no bytes).
    pub fn entry_kind(&mut self, kind: EntryKind) {
        if let Some(e) = self.entries.last_mut() {
            e.kind = kind;
        }
    }

    pub fn append(&mut self, mut data: &[u8]) {
        let entry = self.entries.last_mut().expect("append before begin_entry");
        entry.size += data.len() as u64;
        self.total_size += data.len() as u64;
        self.content_hasher.update(data);
        let bs = self.block_size as usize;
        while !data.is_empty() {
            let room = bs - self.buf.len();
            let n = room.min(data.len());
            self.buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            if self.buf.len() == bs {
                self.emit_block();
            }
        }
    }

    fn emit_block(&mut self) {
        let entry_id = self.entries.last().map(|e| e.entry_id).unwrap_or(0);
        let len = self.buf.len() as u32;
        self.blocks.push(SignatureBlockHash {
            entry_id,
            offset: self.current_offset,
            len,
            weak32: weak::weak32(&self.buf),
            strong_blake3: hash_chunk(&self.buf),
        });
        self.current_offset += len as u64;
        self.buf.clear();
    }

    fn flush_block(&mut self) {
        if !self.buf.is_empty() {
            self.emit_block();
        }
    }

    pub fn finish(mut self, source_label: &str) -> CavsSignature {
        self.flush_block();
        let leaves: Vec<ChunkHash> = self.blocks.iter().map(|b| b.strong_blake3).collect();
        CavsSignature {
            kind: self.kind,
            created_at_unix_ms: 0,
            source_label: source_label.to_string(),
            source_size: self.total_size,
            source_blake3: Some(self.content_hasher.finalize()),
            chunker_profile: self.profile,
            block_size: self.block_size,
            entries: self.entries,
            blocks: self.blocks,
            merkle_root: merkle_root(&leaves),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let mut state = seed;
        for b in out.iter_mut() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (state >> 24) as u8;
        }
        out
    }

    fn sig_of(data: &[u8]) -> CavsSignature {
        let mut b =
            SignatureBuilder::new(SignatureKind::SingleArtifact, DEFAULT_BLOCK_SIZE, "fixed");
        b.begin_entry("test.bin", false, None);
        b.append(data);
        b.finish("test.bin")
    }

    #[test]
    fn encode_decode_roundtrip() {
        let data = pseudo_random(200 * 1024 + 17, 3);
        let sig = sig_of(&data);
        assert_eq!(sig.blocks.len(), 4); // 3 full 64K blocks + short tail
        assert_eq!(sig.blocks[3].len as usize, 200 * 1024 + 17 - 3 * 64 * 1024);
        let bytes = sig.encode();
        let decoded = CavsSignature::decode(&bytes).unwrap();
        assert_eq!(decoded, sig);
    }

    #[test]
    fn encoding_is_deterministic() {
        let data = pseudo_random(100_000, 9);
        assert_eq!(sig_of(&data).encode(), sig_of(&data).encode());
    }

    #[test]
    fn signature_is_compact() {
        // AC5: at least 99% smaller than the source for large inputs.
        let data = pseudo_random(8 * 1024 * 1024, 4);
        let bytes = sig_of(&data).encode();
        assert!(
            (bytes.len() as f64) < data.len() as f64 * 0.01,
            "signature too large: {} bytes for {} input",
            bytes.len(),
            data.len()
        );
    }

    #[test]
    fn corruption_is_rejected() {
        let data = pseudo_random(300_000, 5);
        let good = sig_of(&data).encode();
        // Flip one byte anywhere: the integrity trailer must catch it.
        for pos in [0usize, 9, 64, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[pos] ^= 0xff;
            assert!(
                CavsSignature::decode(&bad).is_err(),
                "corruption at {pos} was accepted"
            );
        }
        // Truncation too.
        assert!(CavsSignature::decode(&good[..good.len() - 10]).is_err());
        assert!(CavsSignature::decode(&good[..4]).is_err());
    }

    #[test]
    fn file_sign_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact.bin");
        let data = pseudo_random(500_000, 12);
        std::fs::write(&path, &data).unwrap();

        let sig = CavsSignature::sign_file(&path, DEFAULT_BLOCK_SIZE, "artifact.bin").unwrap();
        assert_eq!(sig.source_size, data.len() as u64);
        sig.verify_against(&path).unwrap();

        // Mutate the source: verify must fail.
        let mut tampered = data.clone();
        tampered[123_456] ^= 0x80;
        std::fs::write(&path, &tampered).unwrap();
        assert!(matches!(
            sig.verify_against(&path),
            Err(SignatureError::SourceMismatch(_))
        ));
    }

    #[test]
    fn dir_sign_covers_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"alpha").unwrap();
        std::fs::write(dir.path().join("sub/b.bin"), pseudo_random(80_000, 1)).unwrap();

        let sig = CavsSignature::sign_dir(dir.path(), DEFAULT_BLOCK_SIZE, "build").unwrap();
        assert_eq!(sig.kind, SignatureKind::DirectoryContainer);
        let paths: Vec<&str> = sig.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "sub", "sub/b.bin"]);
        sig.verify_against(dir.path()).unwrap();

        let bytes = sig.encode();
        assert_eq!(CavsSignature::decode(&bytes).unwrap(), sig);
    }
}
