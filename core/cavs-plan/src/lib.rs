//! CAVS offline reconstruction plans v1 (`.cavsplan`).
//!
//! A plan is a deterministic, self-verifying description of how to turn an
//! *old* build (single artifact or directory tree) into a *new* one:
//! COPY ranges that reuse old bytes, INLINE data for what changed, plus
//! directory metadata (created dirs, symlinks, executable bits, deletions).
//! It is produced offline from the new build and the old build's
//! `.cavssig` — the old bytes themselves are not required to diff.
//!
//! Two kinds:
//! - **analysis**: ops and estimates only, no payload — for previews,
//!   reports and benchmarks.
//! - **portable**: carries the inline payload (zstd), so `cavs apply` can
//!   reconstruct the new build offline with just the old install.
//!
//! Wire layout (LEB128 varints unless noted):
//!
//! ```text
//! [8]  magic  "CAVSPLN1"
//! u16  version = 1                     (LE)
//! u8   kind                            (1 analysis, 2 portable)
//! u8   mode                            (1 artifact, 2 directory)
//! var  block_size                      (of the old signature used to diff)
//! str  old_label; var old_size; u8 has_old_blake3; [32] if set
//! str  new_label; var new_size
//! var  old_entry_count × { var id; str path; var size }
//! var  new_entry_count × { var id; str path; u8 kind; var size;
//!                          u8 executable; str symlink_target;
//!                          u8 has_blake3; [32] if set }
//! var  deleted_count   × { str path }
//! var  op_count        × { u8 tag; ... }   (see [`PlanOp`])
//! var  blob_raw_len; var blob_comp_len; [blob]   (zstd; empty in analysis)
//! [32] BLAKE3 of every preceding byte  (integrity trailer)
//! ```
//!
//! Encoding is deterministic: the same logical plan always produces the
//! same bytes, so plans can be diffed, cached and content-addressed.
//! Paths are UTF-8 with forward slashes, relative, no `..` components —
//! the decoder rejects anything else (`CAVS-E-PATH-TRAVERSAL`).

pub mod apply;

use cavs_hash::{hash_chunk, ChunkHash};
use cavs_signature::diff::{diff_bytes, DiffOp, WeakHashIndex};
use cavs_signature::{CavsSignature, EntryKind, SignatureKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const PLAN_MAGIC: [u8; 8] = *b"CAVSPLN1";
pub const PLAN_VERSION: u16 = 1;
/// Sanity caps mirroring `.cavssig`: reject hostile counts pre-allocation.
pub const MAX_ENTRIES: u64 = 1 << 24;
pub const MAX_OPS: u64 = 1 << 28;
/// Inline payloads decompress to at most this (a plan larger than 16 GiB
/// of fresh data is not a patch, it's a download).
pub const MAX_BLOB_RAW: u64 = 16 << 30;

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("bad .cavsplan magic")]
    BadMagic,
    #[error("unsupported .cavsplan version {0}")]
    UnsupportedVersion(u16),
    #[error("truncated .cavsplan: {0}")]
    Truncated(&'static str),
    #[error("malformed .cavsplan: {0}")]
    Malformed(&'static str),
    #[error(".cavsplan integrity trailer mismatch (file corrupt)")]
    IntegrityMismatch,
    #[error("invalid plan: {0}")]
    Invalid(String),
    #[error("unsafe path in plan: {0}")]
    PathTraversal(String),
    #[error("plan is analysis-only; a portable plan is required to apply")]
    NotPortable,
    #[error("apply produced wrong output for {0} (hash mismatch, nothing committed)")]
    ApplyHashMismatch(String),
    #[error("apply journal: {0}")]
    Journal(String),
    #[error("symlinks are not supported on this platform: {0}")]
    UnsupportedSymlink(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, PlanError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlanKind {
    /// Ops and estimates only; cannot be applied.
    Analysis = 1,
    /// Carries the inline payload; applies offline.
    Portable = 2,
}

impl PlanKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(PlanKind::Analysis),
            2 => Some(PlanKind::Portable),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PlanKind::Analysis => "analysis",
            PlanKind::Portable => "portable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlanMode {
    Artifact = 1,
    Directory = 2,
}

impl PlanMode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(PlanMode::Artifact),
            2 => Some(PlanMode::Directory),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PlanMode::Artifact => "artifact",
            PlanMode::Directory => "directory",
        }
    }
}

/// An old-build entry a COPY op can read from (files only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldEntry {
    pub entry_id: u32,
    pub path: String,
    pub size: u64,
}

/// One entry of the new build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub entry_id: u32,
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub executable: bool,
    pub symlink_target: Option<String>,
    /// Full BLAKE3 of the file's bytes; the apply-time truth. None for
    /// directories and symlinks.
    pub blake3: Option<ChunkHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOp {
    /// Bytes `new_offset..+len` of new entry equal bytes
    /// `old_offset..+len` of old entry `old_entry_id`.
    CopyOld {
        old_entry_id: u32,
        old_offset: u64,
        new_entry_id: u32,
        new_offset: u64,
        len: u64,
    },
    /// Fresh bytes at `blob_offset..+len` of the (decompressed) payload.
    Inline {
        new_entry_id: u32,
        new_offset: u64,
        len: u64,
        blob_offset: u64,
    },
}

const OP_COPY: u8 = 1;
const OP_INLINE: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflinePlan {
    pub kind: PlanKind,
    pub mode: PlanMode,
    /// Block size of the old signature the plan was diffed against.
    pub block_size: u32,
    pub old_label: String,
    pub old_size: u64,
    pub old_blake3: Option<ChunkHash>,
    pub new_label: String,
    pub new_size: u64,
    pub old_entries: Vec<OldEntry>,
    pub new_entries: Vec<PlanEntry>,
    /// Old paths absent from the new build (managed deletions).
    pub deleted: Vec<String>,
    /// In new-entry order; per entry they tile `0..size` exactly.
    pub ops: Vec<PlanOp>,
    /// Decompressed inline payload (empty for analysis plans).
    pub blob: Vec<u8>,
}

/// Derived numbers for reports and route estimation.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PlanSummary {
    pub ops_total: u64,
    pub copy_ops: u64,
    pub inline_ops: u64,
    pub reused_bytes: u64,
    pub inline_bytes: u64,
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    pub deleted: u64,
    /// Files fully covered by COPY ops from the same old path (no-op
    /// candidates at apply time).
    pub unchanged_files: u64,
}

// ---------------------------------------------------------------------------
// Varint helpers (strict LEB128, same rules as .cavssig)
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
            return Err(PlanError::Truncated("varint"));
        };
        if byte == 0 && shift != 0 {
            return Err(PlanError::Malformed("overlong varint"));
        }
        if i == 9 && byte > 1 {
            return Err(PlanError::Malformed("varint overflow"));
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            *input = &input[i + 1..];
            return Ok(value);
        }
        shift += 7;
    }
    Err(PlanError::Malformed("overlong varint"))
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    write_var(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

fn read_str(input: &mut &[u8]) -> Result<String> {
    let len = read_var(input)? as usize;
    if len > input.len() || len > 1 << 16 {
        return Err(PlanError::Truncated("string"));
    }
    let (head, tail) = input.split_at(len);
    *input = tail;
    String::from_utf8(head.to_vec()).map_err(|_| PlanError::Malformed("string not UTF-8"))
}

fn take<'a>(input: &mut &'a [u8], n: usize, what: &'static str) -> Result<&'a [u8]> {
    if n > input.len() {
        return Err(PlanError::Truncated(what));
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

/// A container-relative path is safe when it is relative, uses forward
/// slashes, has no empty or `.`/`..` components and no drive-style colon.
pub fn path_is_safe(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && !p.contains('\0')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

// ---------------------------------------------------------------------------
// Encode / decode
// ---------------------------------------------------------------------------

impl OfflinePlan {
    pub fn encode(&self, zstd_level: i32) -> Vec<u8> {
        let mut out = Vec::with_capacity(128 + self.ops.len() * 16 + self.blob.len() / 2);
        out.extend_from_slice(&PLAN_MAGIC);
        out.extend_from_slice(&PLAN_VERSION.to_le_bytes());
        out.push(self.kind as u8);
        out.push(self.mode as u8);
        write_var(self.block_size as u64, &mut out);
        write_str(&self.old_label, &mut out);
        write_var(self.old_size, &mut out);
        match &self.old_blake3 {
            Some(h) => {
                out.push(1);
                out.extend_from_slice(h);
            }
            None => out.push(0),
        }
        write_str(&self.new_label, &mut out);
        write_var(self.new_size, &mut out);

        write_var(self.old_entries.len() as u64, &mut out);
        for e in &self.old_entries {
            write_var(e.entry_id as u64, &mut out);
            write_str(&e.path, &mut out);
            write_var(e.size, &mut out);
        }
        write_var(self.new_entries.len() as u64, &mut out);
        for e in &self.new_entries {
            write_var(e.entry_id as u64, &mut out);
            write_str(&e.path, &mut out);
            out.push(e.kind as u8);
            write_var(e.size, &mut out);
            out.push(e.executable as u8);
            write_str(e.symlink_target.as_deref().unwrap_or(""), &mut out);
            match &e.blake3 {
                Some(h) => {
                    out.push(1);
                    out.extend_from_slice(h);
                }
                None => out.push(0),
            }
        }
        write_var(self.deleted.len() as u64, &mut out);
        for p in &self.deleted {
            write_str(p, &mut out);
        }
        write_var(self.ops.len() as u64, &mut out);
        for op in &self.ops {
            match op {
                PlanOp::CopyOld {
                    old_entry_id,
                    old_offset,
                    new_entry_id,
                    new_offset,
                    len,
                } => {
                    out.push(OP_COPY);
                    write_var(*old_entry_id as u64, &mut out);
                    write_var(*old_offset, &mut out);
                    write_var(*new_entry_id as u64, &mut out);
                    write_var(*new_offset, &mut out);
                    write_var(*len, &mut out);
                }
                PlanOp::Inline {
                    new_entry_id,
                    new_offset,
                    len,
                    blob_offset,
                } => {
                    out.push(OP_INLINE);
                    write_var(*new_entry_id as u64, &mut out);
                    write_var(*new_offset, &mut out);
                    write_var(*len, &mut out);
                    write_var(*blob_offset, &mut out);
                }
            }
        }

        let compressed = if self.blob.is_empty() {
            Vec::new()
        } else {
            zstd::bulk::compress(&self.blob, zstd_level).expect("zstd compress cannot fail")
        };
        write_var(self.blob.len() as u64, &mut out);
        write_var(compressed.len() as u64, &mut out);
        out.extend_from_slice(&compressed);

        let trailer = hash_chunk(&out);
        out.extend_from_slice(&trailer);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < PLAN_MAGIC.len() + 2 + 32 {
            return Err(PlanError::Truncated("header"));
        }
        if bytes[..8] != PLAN_MAGIC {
            return Err(PlanError::BadMagic);
        }
        let body_len = bytes.len() - 32;
        let expected: ChunkHash = bytes[body_len..].try_into().unwrap();
        if hash_chunk(&bytes[..body_len]) != expected {
            return Err(PlanError::IntegrityMismatch);
        }

        let mut input = &bytes[8..body_len];
        let version = u16::from_le_bytes(take(&mut input, 2, "version")?.try_into().unwrap());
        if version != PLAN_VERSION {
            return Err(PlanError::UnsupportedVersion(version));
        }
        let kind = PlanKind::from_u8(take(&mut input, 1, "kind")?[0])
            .ok_or(PlanError::Malformed("unknown plan kind"))?;
        let mode = PlanMode::from_u8(take(&mut input, 1, "mode")?[0])
            .ok_or(PlanError::Malformed("unknown plan mode"))?;
        let block_size = read_var(&mut input)?;
        if block_size > u32::MAX as u64 {
            return Err(PlanError::Malformed("block size out of range"));
        }
        let old_label = read_str(&mut input)?;
        let old_size = read_var(&mut input)?;
        let old_blake3 = match take(&mut input, 1, "old hash flag")?[0] {
            0 => None,
            1 => Some(take(&mut input, 32, "old hash")?.try_into().unwrap()),
            _ => return Err(PlanError::Malformed("old hash flag")),
        };
        let new_label = read_str(&mut input)?;
        let new_size = read_var(&mut input)?;

        let old_count = read_var(&mut input)?;
        if old_count > MAX_ENTRIES {
            return Err(PlanError::Malformed("old entry count too large"));
        }
        let mut old_entries = Vec::with_capacity((old_count as usize).min(input.len() / 3));
        for _ in 0..old_count {
            let entry_id = read_var(&mut input)?;
            if entry_id > u32::MAX as u64 {
                return Err(PlanError::Malformed("old entry id out of range"));
            }
            old_entries.push(OldEntry {
                entry_id: entry_id as u32,
                path: read_str(&mut input)?,
                size: read_var(&mut input)?,
            });
        }

        let new_count = read_var(&mut input)?;
        if new_count > MAX_ENTRIES {
            return Err(PlanError::Malformed("new entry count too large"));
        }
        let mut new_entries = Vec::with_capacity((new_count as usize).min(input.len() / 4));
        for _ in 0..new_count {
            let entry_id = read_var(&mut input)?;
            if entry_id > u32::MAX as u64 {
                return Err(PlanError::Malformed("new entry id out of range"));
            }
            let path = read_str(&mut input)?;
            let kind = EntryKind::from_u8(take(&mut input, 1, "entry kind")?[0])
                .ok_or(PlanError::Malformed("unknown entry kind"))?;
            let size = read_var(&mut input)?;
            let executable = match take(&mut input, 1, "exec flag")?[0] {
                0 => false,
                1 => true,
                _ => return Err(PlanError::Malformed("exec flag")),
            };
            let target = read_str(&mut input)?;
            let blake3 = match take(&mut input, 1, "entry hash flag")?[0] {
                0 => None,
                1 => Some(take(&mut input, 32, "entry hash")?.try_into().unwrap()),
                _ => return Err(PlanError::Malformed("entry hash flag")),
            };
            new_entries.push(PlanEntry {
                entry_id: entry_id as u32,
                path,
                kind,
                size,
                executable,
                symlink_target: (!target.is_empty()).then_some(target),
                blake3,
            });
        }

        let deleted_count = read_var(&mut input)?;
        if deleted_count > MAX_ENTRIES {
            return Err(PlanError::Malformed("deleted count too large"));
        }
        let mut deleted = Vec::with_capacity((deleted_count as usize).min(input.len()));
        for _ in 0..deleted_count {
            deleted.push(read_str(&mut input)?);
        }

        let op_count = read_var(&mut input)?;
        if op_count > MAX_OPS {
            return Err(PlanError::Malformed("op count too large"));
        }
        let mut ops = Vec::with_capacity((op_count as usize).min(input.len() / 5));
        for _ in 0..op_count {
            let tag = take(&mut input, 1, "op tag")?[0];
            match tag {
                OP_COPY => {
                    let old_entry_id = read_var(&mut input)?;
                    let old_offset = read_var(&mut input)?;
                    let new_entry_id = read_var(&mut input)?;
                    let new_offset = read_var(&mut input)?;
                    let len = read_var(&mut input)?;
                    if old_entry_id > u32::MAX as u64 || new_entry_id > u32::MAX as u64 {
                        return Err(PlanError::Malformed("op entry id out of range"));
                    }
                    ops.push(PlanOp::CopyOld {
                        old_entry_id: old_entry_id as u32,
                        old_offset,
                        new_entry_id: new_entry_id as u32,
                        new_offset,
                        len,
                    });
                }
                OP_INLINE => {
                    let new_entry_id = read_var(&mut input)?;
                    let new_offset = read_var(&mut input)?;
                    let len = read_var(&mut input)?;
                    let blob_offset = read_var(&mut input)?;
                    if new_entry_id > u32::MAX as u64 {
                        return Err(PlanError::Malformed("op entry id out of range"));
                    }
                    ops.push(PlanOp::Inline {
                        new_entry_id: new_entry_id as u32,
                        new_offset,
                        len,
                        blob_offset,
                    });
                }
                _ => return Err(PlanError::Malformed("unknown op tag")),
            }
        }

        let blob_raw_len = read_var(&mut input)?;
        if blob_raw_len > MAX_BLOB_RAW {
            return Err(PlanError::Malformed("blob too large"));
        }
        let blob_comp_len = read_var(&mut input)? as usize;
        let comp = take(&mut input, blob_comp_len, "blob")?;
        if !input.is_empty() {
            return Err(PlanError::Malformed("trailing bytes"));
        }
        let blob = if blob_raw_len == 0 {
            Vec::new()
        } else {
            let raw = zstd::bulk::decompress(comp, blob_raw_len as usize)
                .map_err(|_| PlanError::Malformed("blob does not decompress"))?;
            if raw.len() as u64 != blob_raw_len {
                return Err(PlanError::Malformed("blob length mismatch"));
            }
            raw
        };

        let plan = OfflinePlan {
            kind,
            mode,
            block_size: block_size as u32,
            old_label,
            old_size,
            old_blake3,
            new_label,
            new_size,
            old_entries,
            new_entries,
            deleted,
            ops,
            blob,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Structural validation: safe paths, known entry ids, exact per-file
    /// output tiling, blob coverage. A decoded plan is always valid; a
    /// hand-built plan should be validated before use.
    pub fn validate(&self) -> Result<()> {
        let mut old_ids = std::collections::HashSet::new();
        for e in &self.old_entries {
            if !old_ids.insert(e.entry_id) {
                return Err(PlanError::Invalid(format!(
                    "duplicate old entry id {}",
                    e.entry_id
                )));
            }
            if self.mode == PlanMode::Directory && !path_is_safe(&e.path) {
                return Err(PlanError::PathTraversal(e.path.clone()));
            }
        }
        let mut new_by_id = HashMap::new();
        for e in &self.new_entries {
            if new_by_id.insert(e.entry_id, e).is_some() {
                return Err(PlanError::Invalid(format!(
                    "duplicate new entry id {}",
                    e.entry_id
                )));
            }
            if self.mode == PlanMode::Directory && !path_is_safe(&e.path) {
                return Err(PlanError::PathTraversal(e.path.clone()));
            }
            if e.kind == EntryKind::File && e.blake3.is_none() {
                return Err(PlanError::Invalid(format!(
                    "file entry {} has no content hash",
                    e.path
                )));
            }
        }
        for p in &self.deleted {
            if !path_is_safe(p) {
                return Err(PlanError::PathTraversal(p.clone()));
            }
        }

        // Per-file tiling: ops appear grouped per entry, offsets contiguous
        // from 0 to the entry size.
        let mut cursor: HashMap<u32, u64> = HashMap::new();
        for op in &self.ops {
            let (new_id, new_offset, len) = match op {
                PlanOp::CopyOld {
                    old_entry_id,
                    new_entry_id,
                    new_offset,
                    len,
                    ..
                } => {
                    if !old_ids.contains(old_entry_id) {
                        return Err(PlanError::Invalid(format!(
                            "op references unknown old entry {old_entry_id}"
                        )));
                    }
                    (*new_entry_id, *new_offset, *len)
                }
                PlanOp::Inline {
                    new_entry_id,
                    new_offset,
                    len,
                    blob_offset,
                } => {
                    if self.kind == PlanKind::Portable
                        && blob_offset
                            .checked_add(*len)
                            .is_none_or(|end| end > self.blob.len() as u64)
                    {
                        return Err(PlanError::Invalid(
                            "inline op reads past the blob".to_string(),
                        ));
                    }
                    (*new_entry_id, *new_offset, *len)
                }
            };
            let Some(entry) = new_by_id.get(&new_id) else {
                return Err(PlanError::Invalid(format!(
                    "op references unknown new entry {new_id}"
                )));
            };
            let at = cursor.entry(new_id).or_insert(0);
            if new_offset != *at {
                return Err(PlanError::Invalid(format!(
                    "gap/overlap in {} at offset {at} (op starts at {new_offset})",
                    entry.path
                )));
            }
            *at += len;
            if *at > entry.size {
                return Err(PlanError::Invalid(format!(
                    "ops overflow {} ({} of {} bytes)",
                    entry.path, at, entry.size
                )));
            }
        }
        for e in &self.new_entries {
            if e.kind == EntryKind::File {
                let got = cursor.get(&e.entry_id).copied().unwrap_or(0);
                if got != e.size {
                    return Err(PlanError::Invalid(format!(
                        "ops cover {got} of {} bytes of {}",
                        e.size, e.path
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> PlanSummary {
        let mut s = PlanSummary {
            ops_total: self.ops.len() as u64,
            deleted: self.deleted.len() as u64,
            ..Default::default()
        };
        let mut copy_bytes_per_entry: HashMap<u32, u64> = HashMap::new();
        for op in &self.ops {
            match op {
                PlanOp::CopyOld {
                    new_entry_id, len, ..
                } => {
                    s.copy_ops += 1;
                    s.reused_bytes += len;
                    *copy_bytes_per_entry.entry(*new_entry_id).or_insert(0) += len;
                }
                PlanOp::Inline { len, .. } => {
                    s.inline_ops += 1;
                    s.inline_bytes += len;
                }
            }
        }
        for e in &self.new_entries {
            match e.kind {
                EntryKind::File => {
                    s.files += 1;
                    if copy_bytes_per_entry.get(&e.entry_id).copied().unwrap_or(0) == e.size
                        && e.size > 0
                    {
                        s.unchanged_files += 1;
                    }
                }
                EntryKind::Directory => s.dirs += 1,
                EntryKind::Symlink => s.symlinks += 1,
            }
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Builder: old signature + new build → plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub kind: PlanKind,
    /// zstd level for the inline payload (patch-quality: 19 by default).
    pub zstd_level: i32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            kind: PlanKind::Portable,
            zstd_level: 19,
        }
    }
}

/// Diff a new build (file or directory) against the old build's signature.
/// Deterministic: same signature + same new bytes ⇒ identical plan bytes.
pub fn build(
    old_sig: &CavsSignature,
    new_source: &Path,
    opts: &BuildOptions,
) -> Result<OfflinePlan> {
    let new_is_dir = new_source.is_dir();
    match (old_sig.kind, new_is_dir) {
        (SignatureKind::SingleArtifact, false) | (SignatureKind::DirectoryContainer, true) => {}
        (SignatureKind::SingleArtifact, true) => {
            return Err(PlanError::Invalid(
                "old signature describes a single artifact but the new build is a directory".into(),
            ))
        }
        (SignatureKind::DirectoryContainer, false) => {
            return Err(PlanError::Invalid(
                "old signature describes a directory but the new build is a single file".into(),
            ))
        }
    }

    let index = WeakHashIndex::build(old_sig);
    let old_entries: Vec<OldEntry> = old_sig
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| OldEntry {
            entry_id: e.entry_id,
            path: e.path.clone(),
            size: e.size,
        })
        .collect();

    let mut plan = OfflinePlan {
        kind: opts.kind,
        mode: if new_is_dir {
            PlanMode::Directory
        } else {
            PlanMode::Artifact
        },
        block_size: old_sig.block_size,
        old_label: old_sig.source_label.clone(),
        old_size: old_sig.source_size,
        old_blake3: old_sig.source_blake3,
        new_label: new_source
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        new_size: 0,
        old_entries,
        new_entries: Vec::new(),
        deleted: Vec::new(),
        ops: Vec::new(),
        blob: Vec::new(),
    };

    let add_file = |plan: &mut OfflinePlan, rel: &str, bytes: &[u8], executable: bool| {
        let entry_id = plan.new_entries.len() as u32;
        let target_path = (plan.mode == PlanMode::Directory).then_some(rel);
        let diff = diff_bytes(&index, bytes, target_path);
        for op in &diff.ops {
            match *op {
                DiffOp::CopyOldRange {
                    entry_id: old_id,
                    old_offset,
                    new_offset,
                    len,
                } => plan.ops.push(PlanOp::CopyOld {
                    old_entry_id: old_id,
                    old_offset,
                    new_entry_id: entry_id,
                    new_offset,
                    len,
                }),
                DiffOp::InlineData { new_offset, len } => {
                    let blob_offset = plan.blob.len() as u64;
                    if opts.kind == PlanKind::Portable {
                        plan.blob.extend_from_slice(
                            &bytes[new_offset as usize..(new_offset + len) as usize],
                        );
                    }
                    plan.ops.push(PlanOp::Inline {
                        new_entry_id: entry_id,
                        new_offset,
                        len,
                        blob_offset,
                    });
                }
            }
        }
        plan.new_entries.push(PlanEntry {
            entry_id,
            path: rel.to_string(),
            kind: EntryKind::File,
            size: bytes.len() as u64,
            executable,
            symlink_target: None,
            blake3: Some(hash_chunk(bytes)),
        });
        plan.new_size += bytes.len() as u64;
    };

    if new_is_dir {
        let mut new_paths = std::collections::HashSet::new();
        for rel in walk_sorted(new_source)? {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if !path_is_safe(&rel_str) {
                return Err(PlanError::PathTraversal(rel_str));
            }
            new_paths.insert(rel_str.clone());
            let full = new_source.join(&rel);
            let meta = std::fs::symlink_metadata(&full)?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&full)?;
                let entry_id = plan.new_entries.len() as u32;
                plan.new_entries.push(PlanEntry {
                    entry_id,
                    path: rel_str,
                    kind: EntryKind::Symlink,
                    size: 0,
                    executable: false,
                    symlink_target: Some(target.to_string_lossy().to_string()),
                    blake3: None,
                });
            } else if meta.is_dir() {
                let entry_id = plan.new_entries.len() as u32;
                plan.new_entries.push(PlanEntry {
                    entry_id,
                    path: rel_str,
                    kind: EntryKind::Directory,
                    size: 0,
                    executable: false,
                    symlink_target: None,
                    blake3: None,
                });
            } else {
                let bytes = std::fs::read(&full)?;
                add_file(&mut plan, &rel_str, &bytes, is_executable(&meta));
            }
        }
        // Managed deletions: every old entry path missing from the new tree.
        for e in &old_sig.entries {
            if !new_paths.contains(&e.path) {
                plan.deleted.push(e.path.clone());
            }
        }
    } else {
        let bytes = std::fs::read(new_source)?;
        let label = plan.new_label.clone();
        add_file(&mut plan, &label, &bytes, false);
    }

    plan.validate()?;
    Ok(plan)
}

/// Deterministic walk: every path under `root`, sorted, symlinks not
/// followed.
fn walk_sorted(root: &Path) -> Result<Vec<PathBuf>> {
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
            let meta = std::fs::symlink_metadata(&child)?;
            out.push(child.strip_prefix(root).unwrap().to_path_buf());
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(child);
            }
        }
    }
    out.sort();
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

#[cfg(test)]
mod tests {
    use super::*;
    use cavs_signature::DEFAULT_BLOCK_SIZE;

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
    fn artifact_plan_roundtrip_and_determinism() {
        let dir = tempfile::tempdir().unwrap();
        let old = pseudo_random(600_000, 1);
        let mut new = old.clone();
        new[300_000..300_500].copy_from_slice(&pseudo_random(500, 2));
        std::fs::write(dir.path().join("old.bin"), &old).unwrap();
        std::fs::write(dir.path().join("new.bin"), &new).unwrap();

        let sig =
            CavsSignature::sign_file(&dir.path().join("old.bin"), DEFAULT_BLOCK_SIZE, "old.bin")
                .unwrap();
        let plan = build(&sig, &dir.path().join("new.bin"), &BuildOptions::default()).unwrap();
        assert_eq!(plan.mode, PlanMode::Artifact);
        let s = plan.summary();
        assert!(
            s.reused_bytes > 400_000,
            "reuse too low: {}",
            s.reused_bytes
        );
        assert!(s.inline_bytes < 200_000);

        let bytes = plan.encode(19);
        let bytes2 = plan.encode(19);
        assert_eq!(bytes, bytes2, "encoding must be deterministic");
        let back = OfflinePlan::decode(&bytes).unwrap();
        assert_eq!(back, plan);
    }

    #[test]
    fn corruption_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let old = pseudo_random(200_000, 3);
        std::fs::write(dir.path().join("old.bin"), &old).unwrap();
        std::fs::write(dir.path().join("new.bin"), &old).unwrap();
        let sig =
            CavsSignature::sign_file(&dir.path().join("old.bin"), DEFAULT_BLOCK_SIZE, "old.bin")
                .unwrap();
        let plan = build(&sig, &dir.path().join("new.bin"), &BuildOptions::default()).unwrap();
        let good = plan.encode(3);
        for pos in [0usize, 10, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[pos] ^= 0xff;
            assert!(OfflinePlan::decode(&bad).is_err(), "corruption at {pos}");
        }
        assert!(OfflinePlan::decode(&good[..good.len() - 5]).is_err());
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        assert!(!path_is_safe("../x"));
        assert!(!path_is_safe("/etc/passwd"));
        assert!(!path_is_safe("a/../b"));
        assert!(!path_is_safe("a\\b"));
        assert!(!path_is_safe("C:/x"));
        assert!(!path_is_safe(""));
        assert!(path_is_safe("a/b/c.bin"));
        assert!(path_is_safe(".cavsignore"));
    }

    #[test]
    fn analysis_plans_have_no_blob_and_cannot_apply() {
        let dir = tempfile::tempdir().unwrap();
        let old = pseudo_random(300_000, 4);
        let new = pseudo_random(300_000, 5);
        std::fs::write(dir.path().join("old.bin"), &old).unwrap();
        std::fs::write(dir.path().join("new.bin"), &new).unwrap();
        let sig =
            CavsSignature::sign_file(&dir.path().join("old.bin"), DEFAULT_BLOCK_SIZE, "old.bin")
                .unwrap();
        let plan = build(
            &sig,
            &dir.path().join("new.bin"),
            &BuildOptions {
                kind: PlanKind::Analysis,
                zstd_level: 3,
            },
        )
        .unwrap();
        assert!(plan.blob.is_empty());
        assert!(plan.summary().inline_bytes > 0);
        let err = apply::apply_artifact(
            &plan,
            &dir.path().join("old.bin"),
            &dir.path().join("out.bin"),
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::NotPortable));
    }
}
