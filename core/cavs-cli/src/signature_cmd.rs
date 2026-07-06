//! `cavs signature` — export, inspect and verify `.cavssig` files
//! (v0.6.0 hybrid reconstruction).

use anyhow::{bail, Context, Result};
use cavs_proto::errors::ErrorCode;
use cavs_signature::{CavsSignature, EntryKind, SignatureBuilder, SignatureKind};
use std::path::Path;

/// Export a signature from a `.cavs` container, a raw file or a directory.
pub fn export(input: &Path, raw: bool, block_size: u32, out: &Path) -> Result<()> {
    let sig = if raw {
        if input.is_dir() {
            CavsSignature::sign_dir(input, block_size, &label_of(input))?
        } else {
            CavsSignature::sign_file(input, block_size, &label_of(input))?
        }
    } else {
        sign_cavs(input, block_size)?
    };
    std::fs::write(out, sig.encode()).with_context(|| format!("cannot write {}", out.display()))?;
    println!(
        "signature: {} -> {} ({} entries, {} blocks of {} KiB, {})",
        input.display(),
        out.display(),
        sig.entries.len(),
        sig.blocks.len(),
        sig.block_size / 1024,
        human_bytes(std::fs::metadata(out)?.len()),
    );
    Ok(())
}

/// Stream every data track of a `.cavs` through the signature builder —
/// the signature then describes the *reconstructed* artifact(s), which is
/// what a client's previous install actually looks like on disk.
fn sign_cavs(input: &Path, block_size: u32) -> Result<CavsSignature> {
    let mut reader = cavs_format::Reader::open(input)
        .map_err(|e| anyhow::anyhow!(ErrorCode::ContainerCorrupt.msg(e)))?;
    let tracks: Vec<_> = reader.tracks().to_vec();
    let data_tracks: Vec<_> = tracks
        .iter()
        .filter(|t| t.kind == cavs_format::TrackKind::Data)
        .collect();
    if data_tracks.is_empty() {
        bail!(
            "{} has no data tracks to sign (media assets are not signable sources)",
            input.display()
        );
    }
    let kind = if data_tracks.len() == 1 {
        SignatureKind::SingleArtifact
    } else {
        SignatureKind::DirectoryContainer
    };
    let mut b = SignatureBuilder::new(kind, block_size, "fixed");
    for track in &data_tracks {
        b.begin_entry(&track.name, false, None);
        let segments: Vec<_> = reader
            .segments_for_track(track.track_id)
            .into_iter()
            .cloned()
            .collect();
        for seg in segments {
            for &c in &seg.chunks {
                let bytes = reader
                    .read_chunk(c)
                    .map_err(|e| anyhow::anyhow!(ErrorCode::ContainerCorrupt.msg(e)))?;
                b.append(&bytes);
            }
        }
    }
    let label = data_tracks
        .first()
        .map(|t| t.name.clone())
        .unwrap_or_default();
    Ok(b.finish(&label))
}

pub fn inspect(input: &Path) -> Result<()> {
    let sig = load(input)?;
    println!("file    : {}", input.display());
    println!("kind    : {}", sig.kind.label());
    println!("label   : {}", sig.source_label);
    println!(
        "source  : {} ({})",
        human_bytes(sig.source_size),
        sig.source_blake3
            .map(|h| cavs_hash::to_hex(&h))
            .unwrap_or_else(|| "no content hash".into())
    );
    println!(
        "blocks  : {} × {} KiB ({} profile)",
        sig.blocks.len(),
        sig.block_size / 1024,
        sig.chunker_profile
    );
    println!("entries : {}", sig.entries.len());
    for e in sig.entries.iter().take(50) {
        let kind = match e.kind {
            EntryKind::File => "file",
            EntryKind::Directory => "dir ",
            EntryKind::Symlink => "link",
        };
        println!(
            "  {kind} {:>12}  {}{}{}",
            human_bytes(e.size),
            e.path,
            if e.executable { " (exec)" } else { "" },
            e.symlink_target
                .as_deref()
                .map(|t| format!(" -> {t}"))
                .unwrap_or_default()
        );
    }
    if sig.entries.len() > 50 {
        println!("  ... and {} more", sig.entries.len() - 50);
    }
    println!("merkle  : {}", cavs_hash::to_hex(&sig.merkle_root));
    Ok(())
}

pub fn verify(input: &Path, against: &Path) -> Result<()> {
    let sig = load(input)?;
    match sig.verify_against(against) {
        Ok(()) => {
            println!(
                "verify  : OK — {} matches the signature ({} blocks)",
                against.display(),
                sig.blocks.len()
            );
            Ok(())
        }
        Err(cavs_signature::SignatureError::SourceMismatch(why)) => {
            bail!("{}", ErrorCode::SignatureMismatch.msg(why))
        }
        Err(e) => bail!("{}", ErrorCode::SignatureCorrupt.msg(e)),
    }
}

pub fn load(path: &Path) -> Result<CavsSignature> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    CavsSignature::decode(&bytes).map_err(|e| anyhow::anyhow!(ErrorCode::SignatureCorrupt.msg(e)))
}

fn label_of(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}
