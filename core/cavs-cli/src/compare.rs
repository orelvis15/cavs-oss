//! Shared classification of a live build against a `.cavssig`: which
//! entries are NEW / MODIFIED / DELETED / SAME. Used by `cavs preview`
//! and `cavs verify-install`.

use anyhow::{bail, Context, Result};
use cavs_signature::{CavsSignature, EntryKind, SignatureBlockHash, SignatureKind};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileState {
    /// Present in the build, absent from the signature.
    New,
    /// Present in both, content differs.
    Modified,
    /// Present in the signature, absent from the build.
    Deleted,
    Same,
}

impl FileState {
    pub fn label(self) -> &'static str {
        match self {
            FileState::New => "NEW",
            FileState::Modified => "MODIFIED",
            FileState::Deleted => "DELETED",
            FileState::Same => "SAME",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryReport {
    pub path: String,
    pub state: FileState,
    /// Size on the build side (0 for DELETED).
    pub size: u64,
}

/// Classify `source` (file or directory, matching the signature kind)
/// against `sig`. Directories and symlinks are compared structurally;
/// files by size + per-block BLAKE3.
pub fn classify(sig: &CavsSignature, source: &Path) -> Result<Vec<EntryReport>> {
    let blocks_by_entry = index_blocks(sig);
    match sig.kind {
        SignatureKind::SingleArtifact => {
            if source.is_dir() {
                bail!(
                    "the signature describes a single artifact but {} is a directory",
                    source.display()
                );
            }
            let entry = sig
                .entries
                .iter()
                .find(|e| e.kind == EntryKind::File)
                .context("signature has no file entry")?;
            let size = std::fs::metadata(source)?.len();
            let state = if file_matches_blocks(
                source,
                size,
                entry.size,
                sig.block_size,
                blocks_by_entry.get(&entry.entry_id).map(Vec::as_slice),
            )? {
                FileState::Same
            } else {
                FileState::Modified
            };
            Ok(vec![EntryReport {
                path: source
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                state,
                size,
            }])
        }
        SignatureKind::DirectoryContainer => {
            if !source.is_dir() {
                bail!(
                    "the signature describes a directory but {} is a file",
                    source.display()
                );
            }
            classify_dir(sig, source, &blocks_by_entry)
        }
    }
}

fn classify_dir(
    sig: &CavsSignature,
    root: &Path,
    blocks_by_entry: &HashMap<u32, Vec<SignatureBlockHash>>,
) -> Result<Vec<EntryReport>> {
    let old_by_path: HashMap<&str, &cavs_signature::SignatureEntry> =
        sig.entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for rel in walk_sorted(root)? {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        // The apply machinery's own files never count against a build.
        if rel_str.starts_with(".cavs-staging") || rel_str == ".cavs-journal.json" {
            continue;
        }
        seen.insert(rel_str.clone());
        let full = root.join(&rel);
        let meta = std::fs::symlink_metadata(&full)?;
        let (kind, size) = if meta.file_type().is_symlink() {
            (EntryKind::Symlink, 0)
        } else if meta.is_dir() {
            (EntryKind::Directory, 0)
        } else {
            (EntryKind::File, meta.len())
        };
        let state = match old_by_path.get(rel_str.as_str()) {
            None => FileState::New,
            Some(old) if old.kind != kind => FileState::Modified,
            Some(old) => match kind {
                EntryKind::Directory => FileState::Same,
                EntryKind::Symlink => {
                    let target = std::fs::read_link(&full)?;
                    if old.symlink_target.as_deref() == Some(target.to_string_lossy().as_ref()) {
                        FileState::Same
                    } else {
                        FileState::Modified
                    }
                }
                EntryKind::File => {
                    if file_matches_blocks(
                        &full,
                        size,
                        old.size,
                        sig.block_size,
                        blocks_by_entry.get(&old.entry_id).map(Vec::as_slice),
                    )? {
                        FileState::Same
                    } else {
                        FileState::Modified
                    }
                }
            },
        };
        // Only report directories when they are new or changed (noise
        // otherwise); files and symlinks always.
        if kind != EntryKind::Directory || state != FileState::Same {
            out.push(EntryReport {
                path: rel_str,
                state,
                size,
            });
        }
    }
    for e in &sig.entries {
        if !seen.contains(&e.path) {
            out.push(EntryReport {
                path: e.path.clone(),
                state: FileState::Deleted,
                size: 0,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn index_blocks(sig: &CavsSignature) -> HashMap<u32, Vec<SignatureBlockHash>> {
    let mut map: HashMap<u32, Vec<SignatureBlockHash>> = HashMap::new();
    for b in &sig.blocks {
        map.entry(b.entry_id).or_default().push(*b);
    }
    map
}

/// Compare a file against an old entry's block hashes without loading the
/// whole file: stream block-by-block, stop at the first difference.
fn file_matches_blocks(
    path: &Path,
    size: u64,
    old_size: u64,
    block_size: u32,
    old_blocks: Option<&[SignatureBlockHash]>,
) -> Result<bool> {
    if size != old_size {
        return Ok(false);
    }
    if size == 0 {
        return Ok(true);
    }
    let Some(blocks) = old_blocks else {
        return Ok(false);
    };
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut buf = vec![0u8; block_size as usize];
    for b in blocks {
        let want = b.len as usize;
        let mut got = 0;
        while got < want {
            let n = file.read(&mut buf[got..want])?;
            if n == 0 {
                return Ok(false);
            }
            got += n;
        }
        if cavs_hash::hash_chunk(&buf[..want]) != b.strong_blake3 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn walk_sorted(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
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
