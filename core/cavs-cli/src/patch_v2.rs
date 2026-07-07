//! `.cavspatch` v2 — optimized pairwise patches with per-file strategy
//! selection (v0.8.0).
//!
//! Where v1 wrapped one whole-artifact external delta, v2 describes a
//! directory (or artifact) transition file by file and picks the best
//! strategy *per file* by measuring real candidate sizes:
//!
//! - `copy-old` — unchanged or renamed/moved file: zero payload;
//! - `plan-ops` — CAVS block-level copy ranges + inline data (wins on
//!   shifted/insert-heavy binaries; streaming apply);
//! - `bsdiff` — external byte-level delta (wins on small binary
//!   mutations; apply loads old+new in memory);
//! - `xdelta3` — external byte-level delta (wins on compressed/high
//!   entropy blobs; windowed apply);
//! - `full-data` — recompressed whole file (new files, nothing to reuse).
//!
//! Payload sections are compressed independently (zstd-19 / brotli-9,
//! whichever is smaller under `--compression auto`) and carry their own
//! BLAKE3; every reconstructed file is verified against its recorded hash
//! before anything is committed. Sidecars serve exactly one old→new pair:
//! generate them only for hot pairs (see the patch policy), never all
//! O(N²) combinations.
//!
//! Wire layout (strict LEB128 varints; hashes raw BLAKE3-256):
//!
//! ```text
//! [8]  magic "CAVSPCH2"; u16 version = 2 (LE); u8 mode (1 artifact, 2 dir)
//! str  old_label; var old_total_size
//! str  new_label; var new_total_size
//! var  old_count × { str path; var size; [32] blake3 }
//! var  new_count × { str path; u8 kind; var size; u8 exec;
//!                    str symlink_target; u8 has_hash; [32] if set;
//!                    u8 strategy_tag; strategy fields }
//!      strategy 1 copy-old  { var old_idx }
//!      strategy 2 plan-ops  { var section; var op_count ×
//!                             { u8 1: var old_idx, var old_off, var len
//!                             | u8 2: var len } }        (inline: sequential)
//!      strategy 3 bsdiff    { var old_idx; var section }
//!      strategy 4 xdelta3   { var old_idx; var section }
//!      strategy 5 full-data { var section }
//! var  deleted_count × { str path }
//! var  section_count × { str compression; var raw_len; var comp_len;
//!                        [32] raw_blake3; [comp bytes] }
//! [32] BLAKE3 integrity trailer over every preceding byte
//! ```

use crate::blob_detect::classify_blob;
use crate::optimize_patch::{compress, missing, run_apply_tool, run_diff_tool};
use crate::report::human_bytes;
use anyhow::{bail, Context, Result};
use cavs_hash::{hash_chunk, ChunkHash, Hasher};
use cavs_proto::errors::ErrorCode;
use cavs_signature::diff::{diff_bytes, DiffOp, WeakHashIndex};
use cavs_signature::{CavsSignature, EntryKind};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PATCH2_MAGIC: [u8; 8] = *b"CAVSPCH2";
pub const PATCH2_VERSION: u16 = 2;
const MAX_ENTRIES: u64 = 1 << 24;
const MAX_OPS: u64 = 1 << 28;
/// bsdiff needs ~(old + new + patch) in memory at diff *and* apply time;
/// above this per-file size the candidate is skipped, not risked.
const BSDIFF_MAX_FILE: u64 = 512 << 20;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    Artifact = 1,
    Directory = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldFile {
    pub path: String,
    pub size: u64,
    pub blake3: ChunkHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    CopyOld {
        old_idx: u32,
        old_offset: u64,
        len: u64,
    },
    /// Consumed sequentially from the entry's section.
    Inline { len: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Strategy {
    /// Whole old file (same path: unchanged; different path: rename/move).
    CopyOld {
        old_idx: u32,
    },
    PlanOps {
        section: u32,
        ops: Vec<FileOp>,
    },
    Bsdiff {
        old_idx: u32,
        section: u32,
    },
    Xdelta3 {
        old_idx: u32,
        section: u32,
    },
    FullData {
        section: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEntry {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub executable: bool,
    pub symlink_target: Option<String>,
    pub blake3: Option<ChunkHash>,
    /// Files only.
    pub strategy: Option<Strategy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub compression: String,
    pub raw_len: u64,
    pub raw_blake3: ChunkHash,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchV2 {
    pub mode: PatchMode,
    pub old_label: String,
    pub old_total_size: u64,
    pub new_label: String,
    pub new_total_size: u64,
    pub old_files: Vec<OldFile>,
    pub new_entries: Vec<NewEntry>,
    pub deleted: Vec<String>,
    pub sections: Vec<Section>,
}

/// Why each file got its strategy — the input of `--explain-strategies`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Decision {
    pub path: String,
    pub size: u64,
    pub shape: String,
    /// Block-level reuse against the old build (plan candidate), 0-100.
    pub reuse_pct: u8,
    /// (candidate label, payload bytes) — every candidate actually measured.
    pub candidates: Vec<(String, u64)>,
    pub chosen: String,
    pub payload_bytes: u64,
    pub why: String,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct GenerateReport {
    pub patch_bytes: u64,
    pub gen_ms: u64,
    pub old_total_size: u64,
    pub new_total_size: u64,
    pub files_total: u64,
    pub files_copy_old: u64,
    pub files_plan_ops: u64,
    pub files_bsdiff: u64,
    pub files_xdelta3: u64,
    pub files_full_data: u64,
    pub renames_detected: u64,
    pub deleted: u64,
    pub skipped_tools: Vec<String>,
    pub decisions: Vec<Decision>,
}

// ---------------------------------------------------------------------------
// Varint / string helpers (same strict rules as .cavsplan)
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
            bail!("{}", ErrorCode::PatchCorrupt.msg("truncated varint"));
        };
        if byte == 0 && shift != 0 {
            bail!("{}", ErrorCode::PatchCorrupt.msg("overlong varint"));
        }
        if i == 9 && byte > 1 {
            bail!("{}", ErrorCode::PatchCorrupt.msg("varint overflow"));
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            *input = &input[i + 1..];
            return Ok(value);
        }
        shift += 7;
    }
    bail!("{}", ErrorCode::PatchCorrupt.msg("overlong varint"))
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    write_var(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

fn read_str(input: &mut &[u8]) -> Result<String> {
    let len = read_var(input)? as usize;
    if len > input.len() || len > 1 << 16 {
        bail!("{}", ErrorCode::PatchCorrupt.msg("truncated string"));
    }
    let (head, tail) = input.split_at(len);
    *input = tail;
    String::from_utf8(head.to_vec())
        .map_err(|_| anyhow::anyhow!(ErrorCode::PatchCorrupt.msg("string not UTF-8")))
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if n > input.len() {
        bail!("{}", ErrorCode::PatchCorrupt.msg("truncated"));
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

// ---------------------------------------------------------------------------
// Encode / decode
// ---------------------------------------------------------------------------

impl PatchV2 {
    pub fn encode(&self) -> Vec<u8> {
        let payload: usize = self.sections.iter().map(|s| s.data.len()).sum();
        let mut out = Vec::with_capacity(payload + 256 + self.new_entries.len() * 48);
        out.extend_from_slice(&PATCH2_MAGIC);
        out.extend_from_slice(&PATCH2_VERSION.to_le_bytes());
        out.push(self.mode as u8);
        write_str(&self.old_label, &mut out);
        write_var(self.old_total_size, &mut out);
        write_str(&self.new_label, &mut out);
        write_var(self.new_total_size, &mut out);

        write_var(self.old_files.len() as u64, &mut out);
        for f in &self.old_files {
            write_str(&f.path, &mut out);
            write_var(f.size, &mut out);
            out.extend_from_slice(&f.blake3);
        }

        write_var(self.new_entries.len() as u64, &mut out);
        for e in &self.new_entries {
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
            match &e.strategy {
                None => out.push(0),
                Some(Strategy::CopyOld { old_idx }) => {
                    out.push(1);
                    write_var(*old_idx as u64, &mut out);
                }
                Some(Strategy::PlanOps { section, ops }) => {
                    out.push(2);
                    write_var(*section as u64, &mut out);
                    write_var(ops.len() as u64, &mut out);
                    for op in ops {
                        match op {
                            FileOp::CopyOld {
                                old_idx,
                                old_offset,
                                len,
                            } => {
                                out.push(1);
                                write_var(*old_idx as u64, &mut out);
                                write_var(*old_offset, &mut out);
                                write_var(*len, &mut out);
                            }
                            FileOp::Inline { len } => {
                                out.push(2);
                                write_var(*len, &mut out);
                            }
                        }
                    }
                }
                Some(Strategy::Bsdiff { old_idx, section }) => {
                    out.push(3);
                    write_var(*old_idx as u64, &mut out);
                    write_var(*section as u64, &mut out);
                }
                Some(Strategy::Xdelta3 { old_idx, section }) => {
                    out.push(4);
                    write_var(*old_idx as u64, &mut out);
                    write_var(*section as u64, &mut out);
                }
                Some(Strategy::FullData { section }) => {
                    out.push(5);
                    write_var(*section as u64, &mut out);
                }
            }
        }

        write_var(self.deleted.len() as u64, &mut out);
        for p in &self.deleted {
            write_str(p, &mut out);
        }

        write_var(self.sections.len() as u64, &mut out);
        for s in &self.sections {
            write_str(&s.compression, &mut out);
            write_var(s.raw_len, &mut out);
            write_var(s.data.len() as u64, &mut out);
            out.extend_from_slice(&s.raw_blake3);
            out.extend_from_slice(&s.data);
        }

        let trailer = hash_chunk(&out);
        out.extend_from_slice(&trailer);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 + 2 + 1 + 32 || bytes[..8] != PATCH2_MAGIC {
            bail!("{}", ErrorCode::PatchCorrupt.msg("not a .cavspatch v2"));
        }
        let body_len = bytes.len() - 32;
        let expected: ChunkHash = bytes[body_len..].try_into().unwrap();
        if hash_chunk(&bytes[..body_len]) != expected {
            bail!(
                "{}",
                ErrorCode::PatchCorrupt.msg(".cavspatch integrity trailer mismatch")
            );
        }
        let mut input = &bytes[8..body_len];
        let version = u16::from_le_bytes(take(&mut input, 2)?.try_into().unwrap());
        if version != PATCH2_VERSION {
            bail!(
                "{}",
                ErrorCode::PatchCorrupt.msg(format!("unsupported .cavspatch version {version}"))
            );
        }
        let mode = match take(&mut input, 1)?[0] {
            1 => PatchMode::Artifact,
            2 => PatchMode::Directory,
            _ => bail!("{}", ErrorCode::PatchCorrupt.msg("unknown patch mode")),
        };
        let old_label = read_str(&mut input)?;
        let old_total_size = read_var(&mut input)?;
        let new_label = read_str(&mut input)?;
        let new_total_size = read_var(&mut input)?;

        let old_count = read_var(&mut input)?;
        if old_count > MAX_ENTRIES {
            bail!("{}", ErrorCode::PatchCorrupt.msg("old entry count"));
        }
        let mut old_files = Vec::with_capacity((old_count as usize).min(input.len() / 34));
        for _ in 0..old_count {
            old_files.push(OldFile {
                path: read_str(&mut input)?,
                size: read_var(&mut input)?,
                blake3: take(&mut input, 32)?.try_into().unwrap(),
            });
        }

        let new_count = read_var(&mut input)?;
        if new_count > MAX_ENTRIES {
            bail!("{}", ErrorCode::PatchCorrupt.msg("new entry count"));
        }
        let mut new_entries = Vec::with_capacity((new_count as usize).min(input.len() / 8));
        for _ in 0..new_count {
            let path = read_str(&mut input)?;
            let kind = EntryKind::from_u8(take(&mut input, 1)?[0])
                .ok_or_else(|| anyhow::anyhow!(ErrorCode::PatchCorrupt.msg("entry kind")))?;
            let size = read_var(&mut input)?;
            let executable = take(&mut input, 1)?[0] == 1;
            let target = read_str(&mut input)?;
            let blake3 = match take(&mut input, 1)?[0] {
                0 => None,
                1 => Some(take(&mut input, 32)?.try_into().unwrap()),
                _ => bail!("{}", ErrorCode::PatchCorrupt.msg("hash flag")),
            };
            let strategy = match take(&mut input, 1)?[0] {
                0 => None,
                1 => Some(Strategy::CopyOld {
                    old_idx: read_var(&mut input)? as u32,
                }),
                2 => {
                    let section = read_var(&mut input)? as u32;
                    let op_count = read_var(&mut input)?;
                    if op_count > MAX_OPS {
                        bail!("{}", ErrorCode::PatchCorrupt.msg("op count"));
                    }
                    let mut ops = Vec::with_capacity((op_count as usize).min(input.len() / 3));
                    for _ in 0..op_count {
                        match take(&mut input, 1)?[0] {
                            1 => ops.push(FileOp::CopyOld {
                                old_idx: read_var(&mut input)? as u32,
                                old_offset: read_var(&mut input)?,
                                len: read_var(&mut input)?,
                            }),
                            2 => ops.push(FileOp::Inline {
                                len: read_var(&mut input)?,
                            }),
                            _ => bail!("{}", ErrorCode::PatchCorrupt.msg("op tag")),
                        }
                    }
                    Some(Strategy::PlanOps { section, ops })
                }
                3 => Some(Strategy::Bsdiff {
                    old_idx: read_var(&mut input)? as u32,
                    section: read_var(&mut input)? as u32,
                }),
                4 => Some(Strategy::Xdelta3 {
                    old_idx: read_var(&mut input)? as u32,
                    section: read_var(&mut input)? as u32,
                }),
                5 => Some(Strategy::FullData {
                    section: read_var(&mut input)? as u32,
                }),
                _ => bail!("{}", ErrorCode::PatchCorrupt.msg("strategy tag")),
            };
            new_entries.push(NewEntry {
                path,
                kind,
                size,
                executable,
                symlink_target: (!target.is_empty()).then_some(target),
                blake3,
                strategy,
            });
        }

        let deleted_count = read_var(&mut input)?;
        if deleted_count > MAX_ENTRIES {
            bail!("{}", ErrorCode::PatchCorrupt.msg("deleted count"));
        }
        let mut deleted = Vec::with_capacity((deleted_count as usize).min(input.len()));
        for _ in 0..deleted_count {
            deleted.push(read_str(&mut input)?);
        }

        let section_count = read_var(&mut input)?;
        if section_count > MAX_ENTRIES {
            bail!("{}", ErrorCode::PatchCorrupt.msg("section count"));
        }
        let mut sections = Vec::with_capacity((section_count as usize).min(input.len() / 40));
        for _ in 0..section_count {
            let compression = read_str(&mut input)?;
            let raw_len = read_var(&mut input)?;
            let comp_len = read_var(&mut input)? as usize;
            let raw_blake3: ChunkHash = take(&mut input, 32)?.try_into().unwrap();
            let data = take(&mut input, comp_len)?.to_vec();
            sections.push(Section {
                compression,
                raw_len,
                raw_blake3,
                data,
            });
        }
        if !input.is_empty() {
            bail!("{}", ErrorCode::PatchCorrupt.msg("trailing bytes"));
        }

        let patch = PatchV2 {
            mode,
            old_label,
            old_total_size,
            new_label,
            new_total_size,
            old_files,
            new_entries,
            deleted,
            sections,
        };
        patch.validate()?;
        Ok(patch)
    }

    fn validate(&self) -> Result<()> {
        let n_old = self.old_files.len() as u64;
        let n_sec = self.sections.len() as u64;
        let check_old = |idx: u32| -> Result<()> {
            if (idx as u64) >= n_old {
                bail!("{}", ErrorCode::PatchInvalid.msg("unknown old file index"));
            }
            Ok(())
        };
        let check_sec = |idx: u32| -> Result<()> {
            if (idx as u64) >= n_sec {
                bail!("{}", ErrorCode::PatchInvalid.msg("unknown section index"));
            }
            Ok(())
        };
        for f in &self.old_files {
            if self.mode == PatchMode::Directory && !cavs_plan::path_is_safe(&f.path) {
                bail!("{}", ErrorCode::PathTraversal.msg(f.path.clone()));
            }
        }
        for p in &self.deleted {
            if !cavs_plan::path_is_safe(p) {
                bail!("{}", ErrorCode::PathTraversal.msg(p.clone()));
            }
        }
        for e in &self.new_entries {
            if self.mode == PatchMode::Directory && !cavs_plan::path_is_safe(&e.path) {
                bail!("{}", ErrorCode::PathTraversal.msg(e.path.clone()));
            }
            match (e.kind, &e.strategy, &e.blake3) {
                (EntryKind::File, None, _) => {
                    bail!(
                        "{}",
                        ErrorCode::PatchInvalid.msg(format!("file {} has no strategy", e.path))
                    )
                }
                (EntryKind::File, _, None) => {
                    bail!(
                        "{}",
                        ErrorCode::PatchInvalid.msg(format!("file {} has no hash", e.path))
                    )
                }
                (EntryKind::Directory | EntryKind::Symlink, Some(_), _) => {
                    bail!(
                        "{}",
                        ErrorCode::PatchInvalid.msg(format!("non-file {} has a strategy", e.path))
                    )
                }
                _ => {}
            }
            match &e.strategy {
                Some(Strategy::CopyOld { old_idx }) => check_old(*old_idx)?,
                Some(Strategy::PlanOps { section, ops }) => {
                    check_sec(*section)?;
                    let mut covered = 0u64;
                    for op in ops {
                        match op {
                            FileOp::CopyOld { old_idx, len, .. } => {
                                check_old(*old_idx)?;
                                covered += len;
                            }
                            FileOp::Inline { len } => covered += len,
                        }
                    }
                    if covered != e.size {
                        bail!(
                            "{}",
                            ErrorCode::PatchInvalid.msg(format!(
                                "ops cover {covered} of {} bytes of {}",
                                e.size, e.path
                            ))
                        );
                    }
                }
                Some(Strategy::Bsdiff { old_idx, section })
                | Some(Strategy::Xdelta3 { old_idx, section }) => {
                    check_old(*old_idx)?;
                    check_sec(*section)?;
                }
                Some(Strategy::FullData { section }) => check_sec(*section)?,
                None => {}
            }
        }
        Ok(())
    }

    /// Hex BLAKE3 of the encoded bytes — the journal identity key.
    pub fn identity(bytes: &[u8]) -> String {
        cavs_hash::to_hex(&hash_chunk(bytes))
    }

    /// Estimated peak apply memory in bytes, dominated by the largest
    /// external-delta file (bsdiff apply holds old + new + raw patch).
    pub fn estimated_apply_peak_bytes(&self) -> u64 {
        const STREAM_BASE: u64 = 32 << 20;
        let mut peak = STREAM_BASE;
        for e in &self.new_entries {
            let est = match &e.strategy {
                Some(Strategy::Bsdiff { old_idx, section }) => {
                    self.old_files[*old_idx as usize].size
                        + e.size
                        + self.sections[*section as usize].raw_len
                        + STREAM_BASE
                }
                Some(Strategy::Xdelta3 { .. }) => 96 << 20,
                _ => STREAM_BASE,
            };
            peak = peak.max(est);
        }
        peak
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GenerateOptions {
    /// auto | plan | bsdiff | xdelta3 | full
    pub algo: String,
    /// auto | zstd-N | brotli-N | none
    pub compression: String,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        GenerateOptions {
            algo: "auto".into(),
            compression: "auto".into(),
        }
    }
}

struct SectionBuilder {
    compression: String,
    brotli_available: bool,
    sections: Vec<Section>,
}

impl SectionBuilder {
    fn new(compression: &str) -> Self {
        SectionBuilder {
            compression: compression.to_string(),
            brotli_available: compression == "auto" && crate::tool_metrics::available("brotli"),
            sections: Vec::new(),
        }
    }

    /// Compress `raw` with the configured (or best) codec; returns the
    /// section index and the compressed size.
    fn push(&mut self, raw: &[u8]) -> Result<(u32, u64)> {
        let (label, data) = if self.compression == "auto" {
            let z = compress(raw, "zstd-19")?;
            if self.brotli_available {
                match compress(raw, "brotli-9") {
                    Ok(b) if b.len() < z.len() => ("brotli-9".to_string(), b),
                    _ => ("zstd-19".to_string(), z),
                }
            } else {
                ("zstd-19".to_string(), z)
            }
        } else {
            (self.compression.clone(), compress(raw, &self.compression)?)
        };
        let idx = self.sections.len() as u32;
        let size = data.len() as u64;
        self.sections.push(Section {
            compression: label,
            raw_len: raw.len() as u64,
            raw_blake3: hash_chunk(raw),
            data,
        });
        Ok((idx, size))
    }

    /// Best-codec compressed size without keeping the section (candidate
    /// probing).
    fn probe(&self, raw: &[u8]) -> Result<u64> {
        let z = compress(
            raw,
            if self.compression == "auto" {
                "zstd-19"
            } else {
                &self.compression
            },
        )?;
        if self.compression == "auto" && self.brotli_available {
            if let Ok(b) = compress(raw, "brotli-9") {
                return Ok(z.len().min(b.len()) as u64);
            }
        }
        Ok(z.len() as u64)
    }
}

/// Generate a v2 patch for an old→new pair (both files, or both dirs).
pub fn generate(
    old: &Path,
    new: &Path,
    opts: &GenerateOptions,
    out: &Path,
) -> Result<GenerateReport> {
    let started = std::time::Instant::now();
    if old.is_dir() != new.is_dir() {
        bail!("--old and --new must both be files or both be directories");
    }
    let mode = if new.is_dir() {
        PatchMode::Directory
    } else {
        PatchMode::Artifact
    };

    // --- Old side: entry table, full-file hashes (rename detection),
    //     block signature (plan candidates). -------------------------------
    let old_label = label_of(old);
    let sig = if old.is_dir() {
        CavsSignature::sign_dir(old, cavs_signature::DEFAULT_BLOCK_SIZE, &old_label)?
    } else {
        CavsSignature::sign_file(old, cavs_signature::DEFAULT_BLOCK_SIZE, &old_label)?
    };
    let index = WeakHashIndex::build(&sig);

    let mut old_files: Vec<OldFile> = Vec::new();
    let mut old_by_path: HashMap<String, u32> = HashMap::new();
    let mut old_by_hash: HashMap<ChunkHash, u32> = HashMap::new();
    // sig entry_id → old_files index, for mapping diff copy ops.
    let mut sig_to_old: HashMap<u32, u32> = HashMap::new();
    for e in &sig.entries {
        if e.kind != EntryKind::File {
            continue;
        }
        let full = if old.is_dir() {
            old.join(&e.path)
        } else {
            old.to_path_buf()
        };
        let hash = hash_file(&full)?;
        let idx = old_files.len() as u32;
        old_files.push(OldFile {
            path: if old.is_dir() {
                e.path.clone()
            } else {
                old_label.clone()
            },
            size: e.size,
            blake3: hash,
        });
        old_by_path.insert(old_files[idx as usize].path.clone(), idx);
        old_by_hash.entry(hash).or_insert(idx);
        sig_to_old.insert(e.entry_id, idx);
    }
    let old_total_size: u64 = old_files.iter().map(|f| f.size).sum();

    // --- New side: choose a strategy per file. ----------------------------
    let mut sections = SectionBuilder::new(&opts.compression);
    let mut report = GenerateReport {
        old_total_size,
        ..Default::default()
    };
    let mut new_entries: Vec<NewEntry> = Vec::new();
    let mut new_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut new_total_size = 0u64;
    let bsdiff_ok = crate::tool_metrics::available("bsdiff");
    let xdelta_ok = crate::tool_metrics::available("xdelta3");
    if !bsdiff_ok {
        report.skipped_tools.push("bsdiff".into());
    }
    if !xdelta_ok {
        report.skipped_tools.push("xdelta3".into());
    }

    let new_files: Vec<(String, PathBuf, std::fs::Metadata)> = if new.is_dir() {
        let mut v = Vec::new();
        for rel in crate::compare::walk_sorted(new)? {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if !cavs_plan::path_is_safe(&rel_str) {
                bail!("{}", ErrorCode::PathTraversal.msg(rel_str));
            }
            let full = new.join(&rel);
            v.push((rel_str, full.clone(), std::fs::symlink_metadata(&full)?));
        }
        v
    } else {
        vec![(
            label_of(new),
            new.to_path_buf(),
            std::fs::symlink_metadata(new)?,
        )]
    };

    for (rel, full, meta) in &new_files {
        new_paths.insert(rel.clone());
        if meta.file_type().is_symlink() {
            new_entries.push(NewEntry {
                path: rel.clone(),
                kind: EntryKind::Symlink,
                size: 0,
                executable: false,
                symlink_target: Some(std::fs::read_link(full)?.to_string_lossy().to_string()),
                blake3: None,
                strategy: None,
            });
            continue;
        }
        if meta.is_dir() {
            new_entries.push(NewEntry {
                path: rel.clone(),
                kind: EntryKind::Directory,
                size: 0,
                executable: false,
                symlink_target: None,
                blake3: None,
                strategy: None,
            });
            continue;
        }

        let bytes = std::fs::read(full).with_context(|| format!("reading {}", full.display()))?;
        let hash = hash_chunk(&bytes);
        new_total_size += bytes.len() as u64;
        report.files_total += 1;

        // 1. Unchanged (same path) or renamed/moved (same content elsewhere):
        //    zero payload. In artifact mode the single old file is the
        //    counterpart whatever the filenames are.
        let same_path = if mode == PatchMode::Artifact {
            (!old_files.is_empty()).then_some(0)
        } else {
            old_by_path.get(rel.as_str()).copied()
        };
        if let Some(idx) = same_path.filter(|&i| old_files[i as usize].blake3 == hash) {
            report.files_copy_old += 1;
            push_file(
                &mut new_entries,
                rel,
                &bytes,
                meta,
                hash,
                Strategy::CopyOld { old_idx: idx },
            );
            continue;
        }
        if let Some(&idx) = old_by_hash.get(&hash) {
            report.files_copy_old += 1;
            report.renames_detected += 1;
            report.decisions.push(Decision {
                path: rel.clone(),
                size: bytes.len() as u64,
                shape: "renamed".into(),
                reuse_pct: 100,
                candidates: vec![("copy-old".into(), 0)],
                chosen: "copy-old".into(),
                payload_bytes: 0,
                why: format!("moved from {} — no payload", old_files[idx as usize].path),
            });
            push_file(
                &mut new_entries,
                rel,
                &bytes,
                meta,
                hash,
                Strategy::CopyOld { old_idx: idx },
            );
            continue;
        }

        // 2. Changed or new: measure candidates, keep the smallest.
        let (shape, magic) = classify_blob(&bytes);
        let shape_label = magic
            .map(|m| m.to_string())
            .unwrap_or_else(|| shape.label().to_string());
        let target = (mode == PatchMode::Directory).then_some(rel.as_str());
        let diff = diff_bytes(&index, &bytes, target);
        let reuse_pct = (diff.reused_bytes * 100 / (bytes.len() as u64).max(1)).min(100) as u8;

        let mut candidates: Vec<(String, u64)> = Vec::new();

        // plan-ops candidate: ops overhead + best-compressed inline bytes.
        let mut inline = Vec::with_capacity(diff.inline_bytes as usize);
        let mut ops: Vec<FileOp> = Vec::new();
        let mut ops_encoded = 0u64;
        let mut plan_valid = true;
        for op in &diff.ops {
            match *op {
                DiffOp::CopyOldRange {
                    entry_id,
                    old_offset,
                    len,
                    ..
                } => {
                    let Some(&old_idx) = sig_to_old.get(&entry_id) else {
                        plan_valid = false;
                        break;
                    };
                    ops.push(FileOp::CopyOld {
                        old_idx,
                        old_offset,
                        len,
                    });
                    ops_encoded += 12;
                }
                DiffOp::InlineData { new_offset, len } => {
                    inline.extend_from_slice(
                        &bytes[new_offset as usize..(new_offset + len) as usize],
                    );
                    ops.push(FileOp::Inline { len });
                    ops_encoded += 5;
                }
            }
        }
        let plan_payload = if plan_valid {
            let inline_comp = if inline.is_empty() {
                0
            } else {
                sections.probe(&inline)?
            };
            candidates.push(("plan-ops".into(), inline_comp + ops_encoded));
            Some((inline, ops, inline_comp + ops_encoded))
        } else {
            None
        };

        // External delta candidates need an old counterpart at the same path.
        let mut bsdiff_raw: Option<Vec<u8>> = None;
        let mut xdelta_raw: Option<Vec<u8>> = None;
        if let Some(old_idx) = same_path {
            let old_full = if old.is_dir() {
                old.join(&old_files[old_idx as usize].path)
            } else {
                old.to_path_buf()
            };
            let old_size = old_files[old_idx as usize].size;
            let want_bsdiff = matches!(opts.algo.as_str(), "auto" | "bsdiff")
                && bsdiff_ok
                && old_size.max(bytes.len() as u64) <= BSDIFF_MAX_FILE;
            if want_bsdiff {
                if let Ok(raw) = run_diff_tool("bsdiff", &old_full, full) {
                    candidates.push(("bsdiff".into(), sections.probe(&raw)?));
                    bsdiff_raw = Some(raw);
                }
            }
            if matches!(opts.algo.as_str(), "auto" | "xdelta3") && xdelta_ok {
                if let Ok(raw) = run_diff_tool("xdelta3", &old_full, full) {
                    candidates.push(("xdelta3".into(), sections.probe(&raw)?));
                    xdelta_raw = Some(raw);
                }
            }
        }

        // full-data candidate: always available.
        let full_comp = sections.probe(&bytes)?;
        candidates.push(("full-data".into(), full_comp));

        // Selection: forced algo, or smallest payload. Ties break toward
        // the lower-memory apply path (plan > xdelta3 > full > bsdiff).
        let order = |label: &str| match label {
            "plan-ops" => 0,
            "xdelta3" => 1,
            "full-data" => 2,
            "bsdiff" => 3,
            _ => 4,
        };
        let chosen_label = match opts.algo.as_str() {
            "auto" => candidates
                .iter()
                .min_by(|a, b| a.1.cmp(&b.1).then(order(&a.0).cmp(&order(&b.0))))
                .map(|(l, _)| l.clone())
                .unwrap(),
            forced => {
                let mapped = if forced == "plan" {
                    "plan-ops"
                } else if forced == "full" {
                    "full-data"
                } else {
                    forced
                };
                if candidates.iter().any(|(l, _)| l == mapped) {
                    mapped.to_string()
                } else {
                    "full-data".to_string()
                }
            }
        };
        let payload_bytes = candidates
            .iter()
            .find(|(l, _)| *l == chosen_label)
            .map(|(_, b)| *b)
            .unwrap_or(0);

        let why = match chosen_label.as_str() {
            "plan-ops" => format!("{reuse_pct}% block reuse; streaming apply"),
            "bsdiff" => format!("byte-level delta wins ({shape_label}, {reuse_pct}% block reuse)"),
            "xdelta3" => format!("byte-level delta wins ({shape_label}, {reuse_pct}% block reuse)"),
            "full-data" => {
                if same_path.is_none() {
                    "new file — nothing to reuse".to_string()
                } else {
                    format!("recompression beats deltas ({shape_label})")
                }
            }
            other => other.to_string(),
        };
        report.decisions.push(Decision {
            path: rel.clone(),
            size: bytes.len() as u64,
            shape: shape_label,
            reuse_pct,
            candidates: candidates.clone(),
            chosen: chosen_label.clone(),
            payload_bytes,
            why,
        });

        let strategy = match chosen_label.as_str() {
            "plan-ops" => {
                report.files_plan_ops += 1;
                let (inline, ops, _) = plan_payload.unwrap();
                let (section, _) = sections.push(&inline)?;
                Strategy::PlanOps { section, ops }
            }
            "bsdiff" => {
                report.files_bsdiff += 1;
                let (section, _) = sections.push(&bsdiff_raw.unwrap())?;
                Strategy::Bsdiff {
                    old_idx: same_path.unwrap(),
                    section,
                }
            }
            "xdelta3" => {
                report.files_xdelta3 += 1;
                let (section, _) = sections.push(&xdelta_raw.unwrap())?;
                Strategy::Xdelta3 {
                    old_idx: same_path.unwrap(),
                    section,
                }
            }
            _ => {
                report.files_full_data += 1;
                let (section, _) = sections.push(&bytes)?;
                Strategy::FullData { section }
            }
        };
        push_file(&mut new_entries, rel, &bytes, meta, hash, strategy);
    }

    // Managed deletions.
    let mut deleted: Vec<String> = Vec::new();
    if mode == PatchMode::Directory {
        for e in &sig.entries {
            if !new_paths.contains(&e.path) {
                deleted.push(e.path.clone());
            }
        }
    }
    report.deleted = deleted.len() as u64;

    let patch = PatchV2 {
        mode,
        old_label,
        old_total_size,
        new_label: label_of(new),
        new_total_size,
        old_files,
        new_entries,
        deleted,
        sections: sections.sections,
    };
    patch.validate()?;
    let encoded = patch.encode();
    std::fs::write(out, &encoded).with_context(|| format!("cannot write {}", out.display()))?;
    report.patch_bytes = encoded.len() as u64;
    report.new_total_size = new_total_size;
    report.gen_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

fn push_file(
    entries: &mut Vec<NewEntry>,
    rel: &str,
    bytes: &[u8],
    meta: &std::fs::Metadata,
    hash: ChunkHash,
    strategy: Strategy,
) {
    entries.push(NewEntry {
        path: rel.to_string(),
        kind: EntryKind::File,
        size: bytes.len() as u64,
        executable: is_executable(meta),
        symlink_target: None,
        blake3: Some(hash),
        strategy: Some(strategy),
    });
}

fn label_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn explain_markdown(report: &GenerateReport) -> String {
    let mut md = String::new();
    md.push_str("# Per-file strategy report\n\n");
    md.push_str(&format!(
        "patch: {} for {} → {} · strategies: {} copy-old ({} renames), {} plan-ops, {} bsdiff, {} xdelta3, {} full-data · {} deletions\n\n",
        human_bytes(report.patch_bytes),
        human_bytes(report.old_total_size),
        human_bytes(report.new_total_size),
        report.files_copy_old,
        report.renames_detected,
        report.files_plan_ops,
        report.files_bsdiff,
        report.files_xdelta3,
        report.files_full_data,
        report.deleted,
    ));
    if !report.skipped_tools.is_empty() {
        md.push_str(&format!(
            "> candidates not measured (tool missing): {}\n\n",
            report.skipped_tools.join(", ")
        ));
    }
    md.push_str(
        "| File | Size | Shape | Block reuse | Chosen | Payload | Candidates measured | Why |\n",
    );
    md.push_str("|---|---:|---|---:|---|---:|---|---|\n");
    for d in &report.decisions {
        let cands = d
            .candidates
            .iter()
            .map(|(l, b)| format!("{l} {}", human_bytes(*b)))
            .collect::<Vec<_>>()
            .join(", ");
        md.push_str(&format!(
            "| {} | {} | {} | {}% | **{}** | {} | {} | {} |\n",
            d.path,
            human_bytes(d.size),
            d.shape,
            d.reuse_pct,
            d.chosen,
            human_bytes(d.payload_bytes),
            cands,
            d.why
        ));
    }
    md
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ApplyV2Options {
    pub delete_removed: bool,
    /// Refuse strategies whose estimated peak memory exceeds this budget.
    pub memory_budget_bytes: Option<u64>,
    /// Verify old files against their recorded hashes before running
    /// external delta tools on them (always cheap-checked by size).
    pub check_old: bool,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ApplyV2Stats {
    pub files_total: u64,
    pub files_written: u64,
    pub files_noop: u64,
    pub deleted: u64,
    pub bytes_written: u64,
    pub elapsed_ms: u64,
    pub estimated_peak_bytes: u64,
}

const PATCH_JOURNAL: &str = ".cavs-journal.json";
const PATCH_STAGING: &str = ".cavs-staging";

#[derive(serde::Serialize, serde::Deserialize)]
struct PatchJournal {
    version: u32,
    patch_blake3: String,
    state: String,
    files_moved: Vec<String>,
}

impl PatchJournal {
    fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(PATCH_JOURNAL);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// Apply a v2 patch. Artifact mode: `out` is the output file. Directory
/// mode: `out` is the output root (may equal `old` for in-place).
pub fn apply(
    patch_path: &Path,
    old: &Path,
    out: &Path,
    opts: &ApplyV2Options,
) -> Result<ApplyV2Stats> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(patch_path)
        .with_context(|| format!("cannot read {}", patch_path.display()))?;
    let patch = PatchV2::decode(&bytes)?;
    let identity = PatchV2::identity(&bytes);

    let mut stats = ApplyV2Stats {
        estimated_peak_bytes: patch.estimated_apply_peak_bytes(),
        ..Default::default()
    };
    if let Some(budget) = opts.memory_budget_bytes {
        if stats.estimated_peak_bytes > budget {
            bail!(
                "{}",
                ErrorCode::MemoryBudgetExceeded.msg(format!(
                    "estimated peak {} exceeds budget {} — use the .cavsplan route \
                     (streaming, ~40 MiB) or raise --memory-budget",
                    human_bytes(stats.estimated_peak_bytes),
                    human_bytes(budget)
                ))
            );
        }
    }

    let tmp = tempfile::tempdir()?;
    match patch.mode {
        PatchMode::Artifact => {
            let entry = patch
                .new_entries
                .iter()
                .find(|e| e.kind == EntryKind::File)
                .ok_or_else(|| anyhow::anyhow!(ErrorCode::PatchInvalid.msg("no output file")))?;
            let part = out.with_extension("cavspatch.part");
            reconstruct_file(&patch, entry, old, &part, tmp.path(), opts)?;
            std::fs::rename(&part, out)?;
            stats.files_total = 1;
            stats.files_written = 1;
            stats.bytes_written = entry.size;
        }
        PatchMode::Directory => {
            if !old.is_dir() {
                bail!("--old must be the installed directory for a directory patch");
            }
            std::fs::create_dir_all(out)?;
            let staging = out.join(PATCH_STAGING);
            std::fs::create_dir_all(&staging)?;
            let mut journal = PatchJournal {
                version: 1,
                patch_blake3: identity,
                state: "staging".into(),
                files_moved: Vec::new(),
            };
            journal.save(out)?;

            // Stage everything first; commit only after all hashes pass.
            let mut staged: Vec<(&NewEntry, PathBuf)> = Vec::new();
            for (i, entry) in patch.new_entries.iter().enumerate() {
                if entry.kind != EntryKind::File {
                    continue;
                }
                stats.files_total += 1;
                let expected = entry.blake3.expect("validated");
                let final_path = out.join(&entry.path);
                if cavs_plan::apply::file_matches(&final_path, entry.size, &expected) {
                    stats.files_noop += 1;
                    continue;
                }
                let staged_path = staging.join(format!("e{i}"));
                if !cavs_plan::apply::file_matches(&staged_path, entry.size, &expected) {
                    if let Err(e) =
                        reconstruct_file(&patch, entry, old, &staged_path, tmp.path(), opts)
                    {
                        journal.state = "failed".into();
                        let _ = journal.save(out);
                        return Err(e);
                    }
                    stats.bytes_written += entry.size;
                }
                staged.push((entry, staged_path));
            }

            journal.state = "verified".into();
            journal.save(out)?;

            for entry in &patch.new_entries {
                if entry.kind == EntryKind::Directory {
                    std::fs::create_dir_all(out.join(&entry.path))?;
                }
            }
            journal.state = "committing".into();
            journal.save(out)?;
            for (entry, staged_path) in &staged {
                let final_path = out.join(&entry.path);
                if let Some(parent) = final_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(staged_path, &final_path)?;
                set_executable(&final_path, entry.executable)?;
                journal.files_moved.push(entry.path.clone());
                stats.files_written += 1;
            }
            for entry in &patch.new_entries {
                if entry.kind == EntryKind::File {
                    set_executable(&out.join(&entry.path), entry.executable)?;
                }
                if entry.kind == EntryKind::Symlink {
                    create_symlink(
                        entry.symlink_target.as_deref().unwrap_or(""),
                        &out.join(&entry.path),
                    )?;
                }
            }
            if opts.delete_removed {
                for p in &patch.deleted {
                    let path = out.join(p);
                    if path.is_dir() {
                        if std::fs::remove_dir(&path).is_ok() {
                            stats.deleted += 1;
                        }
                    } else if path.symlink_metadata().is_ok() {
                        std::fs::remove_file(&path)?;
                        stats.deleted += 1;
                    }
                }
            }
            journal.state = "committed".into();
            journal.save(out)?;
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_file(out.join(PATCH_JOURNAL));
        }
    }
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(stats)
}

/// Reconstruct one file entry into `dest` and verify its hash (deleting
/// the output on mismatch).
fn reconstruct_file(
    patch: &PatchV2,
    entry: &NewEntry,
    old_root: &Path,
    dest: &Path,
    tmp: &Path,
    opts: &ApplyV2Options,
) -> Result<()> {
    let expected = entry.blake3.expect("validated: files carry hashes");
    let old_path = |idx: u32| -> PathBuf {
        let f = &patch.old_files[idx as usize];
        if patch.mode == PatchMode::Artifact {
            old_root.to_path_buf()
        } else {
            old_root.join(&f.path)
        }
    };
    let check_old = |idx: u32| -> Result<PathBuf> {
        let f = &patch.old_files[idx as usize];
        let p = old_path(idx);
        let meta = std::fs::metadata(&p)
            .with_context(|| format!("old file {} is missing", p.display()))?;
        if meta.len() != f.size || (opts.check_old && hash_file(&p)? != f.blake3) {
            bail!(
                "{}",
                ErrorCode::ApplyHashMismatch.msg(format!(
                    "{} is not the old version this patch expects",
                    p.display()
                ))
            );
        }
        Ok(p)
    };

    match entry.strategy.as_ref().expect("validated") {
        Strategy::CopyOld { old_idx } => {
            let src = check_old(*old_idx)?;
            std::fs::copy(&src, dest)?;
        }
        Strategy::FullData { section } => {
            decompress_section_to(&patch.sections[*section as usize], dest)?;
        }
        Strategy::PlanOps { section, ops } => {
            // Inline bytes stream sequentially from the decompressed
            // section (spilled to disk, so peak RAM stays flat).
            let inline_path = tmp.join(format!("s{section}"));
            decompress_section_to(&patch.sections[*section as usize], &inline_path)?;
            let mut inline = std::io::BufReader::new(std::fs::File::open(&inline_path)?);
            let out_file = std::fs::File::create(dest)?;
            let mut writer = std::io::BufWriter::new(out_file);
            let mut open: HashMap<u32, std::fs::File> = HashMap::new();
            let mut buf = vec![0u8; 8 << 20];
            for op in ops {
                match op {
                    FileOp::CopyOld {
                        old_idx,
                        old_offset,
                        len,
                    } => {
                        if !open.contains_key(old_idx) {
                            open.insert(*old_idx, std::fs::File::open(old_path(*old_idx))?);
                        }
                        let f = open.get_mut(old_idx).unwrap();
                        f.seek(SeekFrom::Start(*old_offset))?;
                        let mut done = 0u64;
                        while done < *len {
                            let n = ((*len - done) as usize).min(buf.len());
                            f.read_exact(&mut buf[..n])?;
                            writer.write_all(&buf[..n])?;
                            done += n as u64;
                        }
                    }
                    FileOp::Inline { len } => {
                        let mut done = 0u64;
                        while done < *len {
                            let n = ((*len - done) as usize).min(buf.len());
                            inline.read_exact(&mut buf[..n])?;
                            writer.write_all(&buf[..n])?;
                            done += n as u64;
                        }
                    }
                }
            }
            writer.flush()?;
        }
        Strategy::Bsdiff { old_idx, section } | Strategy::Xdelta3 { old_idx, section } => {
            let algo = if matches!(entry.strategy, Some(Strategy::Bsdiff { .. })) {
                "bsdiff"
            } else {
                "xdelta3"
            };
            let src = check_old(*old_idx)?;
            let patch_tmp = tmp.join(format!("p{section}"));
            decompress_section_to(&patch.sections[*section as usize], &patch_tmp)?;
            run_apply_tool(algo, &src, &patch_tmp, dest)?;
        }
    }

    if hash_file(dest)? != expected {
        let _ = std::fs::remove_file(dest);
        bail!(
            "{}",
            ErrorCode::ApplyHashMismatch.msg(format!(
                "reconstructed {} does not match its recorded hash",
                entry.path
            ))
        );
    }
    Ok(())
}

/// Stream-decompress a section to a file and verify its raw BLAKE3.
fn decompress_section_to(section: &Section, dest: &Path) -> Result<()> {
    let raw = match section.compression.as_str() {
        "none" => section.data.clone(),
        c if c.starts_with("zstd-") => {
            zstd::bulk::decompress(&section.data, section.raw_len as usize).map_err(|_| {
                anyhow::anyhow!(ErrorCode::PatchCorrupt.msg("section does not decompress"))
            })?
        }
        c if c.starts_with("brotli-") => brotli_decompress(&section.data)?,
        other => bail!(
            "{}",
            ErrorCode::PatchCorrupt.msg(format!("unknown compression {other}"))
        ),
    };
    if raw.len() as u64 != section.raw_len || hash_chunk(&raw) != section.raw_blake3 {
        bail!("{}", ErrorCode::PatchCorrupt.msg("section hash mismatch"));
    }
    std::fs::write(dest, raw)?;
    Ok(())
}

fn brotli_decompress(data: &[u8]) -> Result<Vec<u8>> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("brotli")
        .args(["-d", "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| missing("brotli"))?;
    let mut stdin = child.stdin.take().unwrap();
    let input = data.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out)?;
    let _ = writer.join();
    if !child.wait()?.success() {
        bail!("brotli -d failed");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn hash_file(path: &Path) -> Result<ChunkHash> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize())
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path)?;
    let mode = meta.permissions().mode();
    let want = if executable {
        mode | 0o755
    } else {
        mode & !0o111
    };
    if want != mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(want))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, link: &Path) -> Result<()> {
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)?;
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_symlink(_target: &str, link: &Path) -> Result<()> {
    eprintln!(
        "[apply-patch] {}",
        ErrorCode::UnsupportedSymlink.msg(format!("skipping {}", link.display()))
    );
    Ok(())
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

    fn write_tree(root: &Path, files: &[(&str, Vec<u8>)]) {
        for (rel, bytes) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }

    fn tree_files(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for rel in crate::compare::walk_sorted(root).unwrap() {
            let full = root.join(&rel);
            if full.is_file() {
                out.push((
                    rel.to_string_lossy().replace('\\', "/"),
                    std::fs::read(&full).unwrap(),
                ));
            }
        }
        out
    }

    /// Directory generate→apply roundtrip with modified, new, renamed and
    /// deleted files; no external tools required (plan/full candidates).
    #[test]
    fn dir_patch_roundtrip_with_renames() {
        let dir = tempfile::tempdir().unwrap();
        let old_root = dir.path().join("v1");
        let new_root = dir.path().join("v2");
        let big = pseudo_random(400_000, 1);
        let mut big2 = big.clone();
        big2[100_000..101_000].copy_from_slice(&pseudo_random(1000, 2));
        let moved = pseudo_random(120_000, 3);
        write_tree(
            &old_root,
            &[
                ("data/big.bin", big),
                ("assets/level_01.dat", moved.clone()),
                ("old/gone.txt", b"bye".to_vec()),
            ],
        );
        write_tree(
            &new_root,
            &[
                ("data/big.bin", big2),
                ("levels/level_01.dat", moved), // renamed
                ("new/fresh.bin", pseudo_random(90_000, 4)),
            ],
        );

        let out = dir.path().join("p.cavspatch");
        let report = generate(&old_root, &new_root, &GenerateOptions::default(), &out).unwrap();
        assert_eq!(report.renames_detected, 1, "rename must be metadata-only");
        assert!(
            report.patch_bytes < 200_000,
            "patch too big: {}",
            report.patch_bytes
        );

        let rebuilt = dir.path().join("rebuilt");
        crate::bench_butler::copy_tree(&old_root, &rebuilt).unwrap();
        let stats = apply(
            &out,
            &rebuilt,
            &rebuilt,
            &ApplyV2Options {
                delete_removed: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(stats.files_written >= 3);
        assert_eq!(tree_files(&new_root), tree_files(&rebuilt));
        assert!(!rebuilt.join("old/gone.txt").exists());
        assert!(!rebuilt.join(PATCH_STAGING).exists());
        assert!(!rebuilt.join(PATCH_JOURNAL).exists());
    }

    #[test]
    fn artifact_patch_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let old = pseudo_random(500_000, 7);
        let mut new = Vec::from(&pseudo_random(4096, 8)[..]);
        new.extend_from_slice(&old); // shift: plan-ops should win
        let old_p = dir.path().join("v1.pck");
        let new_p = dir.path().join("v2.pck");
        std::fs::write(&old_p, &old).unwrap();
        std::fs::write(&new_p, &new).unwrap();

        let out = dir.path().join("p.cavspatch");
        let report = generate(&old_p, &new_p, &GenerateOptions::default(), &out).unwrap();
        assert!(
            report.patch_bytes < 60_000,
            "shifted artifact patch should be tiny, got {}",
            report.patch_bytes
        );

        let rebuilt = dir.path().join("rebuilt.pck");
        apply(&out, &old_p, &rebuilt, &ApplyV2Options::default()).unwrap();
        assert_eq!(std::fs::read(&rebuilt).unwrap(), new);
    }

    #[test]
    fn corruption_is_rejected_and_nothing_committed() {
        let dir = tempfile::tempdir().unwrap();
        let old_p = dir.path().join("v1.bin");
        let new_p = dir.path().join("v2.bin");
        // Small mutation: the winning strategy must reuse old bytes, so a
        // wrong old input has to be caught by the output hash.
        let old = pseudo_random(200_000, 9);
        let mut new = old.clone();
        new[50_000..51_000].copy_from_slice(&pseudo_random(1000, 10));
        std::fs::write(&old_p, &old).unwrap();
        std::fs::write(&new_p, &new).unwrap();
        let out = dir.path().join("p.cavspatch");
        generate(&old_p, &new_p, &GenerateOptions::default(), &out).unwrap();

        let good = std::fs::read(&out).unwrap();
        for pos in [9usize, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[pos] ^= 0xff;
            assert!(PatchV2::decode(&bad).is_err(), "corruption at {pos}");
        }

        // Wrong old input: apply must fail and leave nothing behind.
        std::fs::write(&old_p, pseudo_random(200_000, 11)).unwrap();
        let rebuilt = dir.path().join("rebuilt.bin");
        assert!(apply(&out, &old_p, &rebuilt, &ApplyV2Options::default()).is_err());
        assert!(!rebuilt.exists());
    }

    #[test]
    fn memory_budget_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let old_p = dir.path().join("v1.bin");
        let new_p = dir.path().join("v2.bin");
        std::fs::write(&old_p, pseudo_random(300_000, 12)).unwrap();
        std::fs::write(&new_p, pseudo_random(300_000, 13)).unwrap();
        let out = dir.path().join("p.cavspatch");
        generate(&old_p, &new_p, &GenerateOptions::default(), &out).unwrap();

        let rebuilt = dir.path().join("rebuilt.bin");
        let err = apply(
            &out,
            &old_p,
            &rebuilt,
            &ApplyV2Options {
                memory_budget_bytes: Some(1),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("CAVS-E-MEMORY-BUDGET-EXCEEDED"));
        // Streaming strategies fit a modest budget.
        apply(
            &out,
            &old_p,
            &rebuilt,
            &ApplyV2Options {
                memory_budget_bytes: Some(256 << 20),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&rebuilt).unwrap(),
            std::fs::read(&new_p).unwrap()
        );
    }
}
