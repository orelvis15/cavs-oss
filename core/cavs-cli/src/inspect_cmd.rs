//! `cavs file` / `cavs ls` — identify any CAVS file type and list what is
//! inside. Unknown or corrupt files fail cleanly with a non-zero exit.

use crate::report::human_bytes;
use anyhow::{bail, Result};
use cavs_plan::OfflinePlan;
use cavs_signature::{CavsSignature, EntryKind};
use std::path::Path;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

enum Detected {
    Container,
    Signature(Box<CavsSignature>),
    Plan(Box<OfflinePlan>),
    Patch,
    Manifest,
    Bootstrap,
}

fn detect(path: &Path, bytes: &[u8]) -> Result<Detected> {
    if bytes.len() >= 8 {
        match &bytes[..8] {
            m if m == cavs_signature::SIGNATURE_MAGIC => {
                return Ok(Detected::Signature(Box::new(
                    CavsSignature::decode(bytes)
                        .map_err(|e| anyhow::anyhow!("corrupt .cavssig: {e}"))?,
                )))
            }
            m if m == cavs_plan::PLAN_MAGIC => {
                return Ok(Detected::Plan(Box::new(
                    OfflinePlan::decode(bytes)
                        .map_err(|e| anyhow::anyhow!("corrupt .cavsplan: {e}"))?,
                )))
            }
            m if m == crate::optimize_patch::PATCH_MAGIC => {
                crate::optimize_patch::decode(bytes)?;
                return Ok(Detected::Patch);
            }
            _ => {}
        }
    }
    if bytes.len() >= 4 && bytes[..4] == cavs_format::MAGIC {
        cavs_format::Reader::open(path).map_err(|e| anyhow::anyhow!("corrupt .cavs: {e}"))?;
        return Ok(Detected::Container);
    }
    if bytes.len() >= 4 && bytes[..4] == ZSTD_MAGIC {
        return Ok(Detected::Bootstrap);
    }
    if cavs_manifest::read_manifest(bytes).is_ok() {
        return Ok(Detected::Manifest);
    }
    bail!(
        "{}: not a CAVS file (unknown or corrupt format)",
        path.display()
    );
}

pub fn file_info(path: &Path, json: bool) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let size = bytes.len() as u64;
    let mut fields: Vec<(&str, String)> = vec![("file", path.display().to_string())];
    match detect(path, &bytes)? {
        Detected::Container => {
            let reader = cavs_format::Reader::open(path)?;
            let tracks = reader.tracks();
            let data_tracks = tracks
                .iter()
                .filter(|t| t.kind == cavs_format::TrackKind::Data)
                .count();
            let payload_dir = reader
                .meta()
                .iter()
                .any(|(k, v)| k == "payload" && v == "directory");
            fields.push(("type", "CAVS container (.cavs)".into()));
            fields.push((
                "mode",
                if payload_dir { "directory" } else { "asset" }.into(),
            ));
            fields.push(("tracks", tracks.len().to_string()));
            fields.push(("data_tracks", data_tracks.to_string()));
            fields.push(("chunks", reader.chunks().len().to_string()));
        }
        Detected::Signature(sig) => {
            fields.push(("type", "CAVS signature (.cavssig)".into()));
            fields.push(("version", "CAVSSIG1".into()));
            fields.push(("mode", sig.kind.label().into()));
            fields.push(("entries", sig.entries.len().to_string()));
            fields.push(("blocks", sig.blocks.len().to_string()));
            fields.push(("block_size", format!("{} KiB", sig.block_size / 1024)));
            fields.push(("source_size", human_bytes(sig.source_size)));
            fields.push((
                "signature_pct_of_source",
                format!(
                    "{:.3}%",
                    size as f64 * 100.0 / sig.source_size.max(1) as f64
                ),
            ));
        }
        Detected::Plan(plan) => {
            let s = plan.summary();
            fields.push(("type", "CAVS reconstruction plan (.cavsplan)".into()));
            fields.push(("version", "CAVSPLN1".into()));
            fields.push(("kind", plan.kind.label().into()));
            fields.push(("mode", plan.mode.label().into()));
            fields.push((
                "old",
                format!("{} ({})", plan.old_label, human_bytes(plan.old_size)),
            ));
            fields.push((
                "new",
                format!("{} ({})", plan.new_label, human_bytes(plan.new_size)),
            ));
            fields.push(("ops", s.ops_total.to_string()));
            fields.push(("reused", human_bytes(s.reused_bytes)));
            fields.push(("fresh", human_bytes(s.inline_bytes)));
            fields.push(("deletions", s.deleted.to_string()));
        }
        Detected::Patch => {
            let (h, payload) = crate::optimize_patch::decode(&bytes)?;
            fields.push((
                "type",
                "CAVS pairwise sidecar (.cavspatch, experimental)".into(),
            ));
            fields.push(("algo", h.algo));
            fields.push(("compression", h.compression));
            fields.push(("old", human_bytes(h.old_size)));
            fields.push(("new", human_bytes(h.new_size)));
            fields.push(("payload", human_bytes(payload.len() as u64)));
        }
        Detected::Manifest => {
            let loaded = cavs_manifest::read_manifest(&bytes).unwrap();
            fields.push(("type", "CAVS manifest".into()));
            fields.push(("format", loaded.format.label().into()));
            fields.push(("asset", loaded.manifest.asset.clone()));
            fields.push(("segments", loaded.manifest.segments.len().to_string()));
        }
        Detected::Bootstrap => {
            fields.push(("type", "zstd stream (CAVS bootstrap artifact)".into()));
        }
    }
    fields.push(("size", human_bytes(size)));

    if json {
        let map: serde_json::Map<String, serde_json::Value> = fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
            .collect();
        println!("{}", serde_json::to_string_pretty(&map)?);
    } else {
        let width = fields.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (k, v) in fields {
            println!("{k:<width$} : {v}");
        }
    }
    Ok(())
}

pub fn ls(path: &Path, json: bool) -> Result<()> {
    let bytes = std::fs::read(path)?;
    #[derive(serde::Serialize)]
    struct Row {
        kind: &'static str,
        size: u64,
        path: String,
    }
    let rows: Vec<Row> = match detect(path, &bytes)? {
        Detected::Signature(sig) => sig
            .entries
            .iter()
            .map(|e| Row {
                kind: kind_label(e.kind),
                size: e.size,
                path: match &e.symlink_target {
                    Some(t) => format!("{} -> {t}", e.path),
                    None => e.path.clone(),
                },
            })
            .collect(),
        Detected::Plan(plan) => {
            let mut rows: Vec<Row> = plan
                .new_entries
                .iter()
                .map(|e| Row {
                    kind: kind_label(e.kind),
                    size: e.size,
                    path: match &e.symlink_target {
                        Some(t) => format!("{} -> {t}", e.path),
                        None => e.path.clone(),
                    },
                })
                .collect();
            rows.extend(plan.deleted.iter().map(|p| Row {
                kind: "delete",
                size: 0,
                path: p.clone(),
            }));
            rows
        }
        Detected::Container => {
            let reader = cavs_format::Reader::open(path)?;
            let tracks: Vec<_> = reader.tracks().to_vec();
            tracks
                .iter()
                .map(|t| {
                    let size: u64 = reader
                        .segments_for_track(t.track_id)
                        .iter()
                        .flat_map(|s| s.chunks.iter())
                        .map(|&c| reader.chunks()[c as usize].len_raw as u64)
                        .sum();
                    Row {
                        kind: match t.kind {
                            cavs_format::TrackKind::Data => "file",
                            _ => "media",
                        },
                        size,
                        path: t.name.clone(),
                    }
                })
                .collect()
        }
        Detected::Manifest => {
            let loaded = cavs_manifest::read_manifest(&bytes).unwrap();
            loaded
                .manifest
                .tracks
                .iter()
                .map(|t| Row {
                    kind: "track",
                    size: 0,
                    path: t.name.clone(),
                })
                .collect()
        }
        Detected::Patch | Detected::Bootstrap => {
            bail!(
                "{}: this file type has no listable entries (try `cavs file`)",
                path.display()
            )
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for r in &rows {
            println!("{:<6} {:>12}  {}", r.kind, human_bytes(r.size), r.path);
        }
        println!("{} entries", rows.len());
    }
    Ok(())
}

fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Directory => "dir",
        EntryKind::Symlink => "link",
    }
}
