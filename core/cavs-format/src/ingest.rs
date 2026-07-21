//! Ingest a `.cavs` container into a [`GlobalStore`], deduplicating its
//! chunks against everything already stored and publishing the asset record.
//!
//! This is the shared library form of what `cavs store add` does; it is also
//! used by `cavs-lfs-agent` to push Git LFS objects into a store-backed
//! remote.

use crate::{Reader, SignatureStatus, SEGMENT_FLAG_RANDOM_ACCESS};
use cavs_hash::to_hex;
use cavs_store::{AssetRecord, GlobalStore, StoreSegment, StoreTrack};

/// Counters from one [`ingest_into_store`] call.
#[derive(Debug, Clone, Copy, Default)]
pub struct IngestStats {
    /// Total unique chunks referenced by the container.
    pub chunks: u64,
    /// Chunks that were new to the store (the rest deduplicated).
    pub new_chunks: u64,
    /// Stored (possibly compressed) bytes written for the new chunks.
    pub new_bytes: u64,
}

/// Ingest an open `.cavs` [`Reader`] into `store` under `asset_name`,
/// deduplicating its chunks against everything already stored.
///
/// Refuses containers whose embedded signature is invalid (propagated from
/// [`Reader::verify_signature`]); unsigned containers are accepted and
/// published without a signature.
pub fn ingest_into_store(
    reader: &mut Reader,
    store: &mut GlobalStore,
    asset_name: &str,
) -> crate::Result<IngestStats> {
    // Refuse to ingest content whose embedded signature is invalid.
    let signature = match reader.verify_signature()? {
        SignatureStatus::Valid(_) => reader.embedded_signature(),
        SignatureStatus::Unsigned => None,
    };

    let chunks = reader.chunks().to_vec();

    // Store every unique chunk (in stored/compressed form) into the CAS.
    let mut stats = IngestStats {
        chunks: chunks.len() as u64,
        ..IngestStats::default()
    };
    for i in 0..chunks.len() as u32 {
        let (stored, flags, len_raw) = reader.read_chunk_stored(i)?;
        let hash = chunks[i as usize].hash;
        if store.put_chunk(&hash, &stored, flags, len_raw)? {
            stats.new_chunks += 1;
            stats.new_bytes += stored.len() as u64;
        }
    }

    let hex = |idx: u32| to_hex(&chunks[idx as usize].hash);
    let tracks: Vec<StoreTrack> = reader
        .tracks()
        .iter()
        .map(|t| StoreTrack {
            track_id: t.track_id,
            kind: t.kind as u8,
            codec: t.codec.clone(),
            name: t.name.clone(),
            timescale: t.timescale,
            init_chunks: t.init_chunks.iter().map(|&i| hex(i)).collect(),
        })
        .collect();
    let segments: Vec<StoreSegment> = reader
        .segments()
        .iter()
        .map(|s| StoreSegment {
            segment_id: s.segment_id,
            track_id: s.track_id,
            pts_start: s.pts_start,
            duration: s.duration,
            random_access: s.flags & SEGMENT_FLAG_RANDOM_ACCESS != 0,
            chunks: s.chunks.iter().map(|&i| hex(i)).collect(),
        })
        .collect();

    let record = AssetRecord {
        name: asset_name.to_string(),
        asset_uuid: reader
            .superblock()
            .asset_uuid
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        tracks,
        segments,
        dict: reader.dict().iter().map(|&i| hex(i)).collect(),
        chunk_table: chunks.iter().map(|c| to_hex(&c.hash)).collect(),
        merkle_root: to_hex(&reader.integrity().merkle_root),
        signature: signature.map(|(sig, _)| {
            to_hex(&sig[..32].try_into().unwrap()) + &to_hex(&sig[32..].try_into().unwrap())
        }),
        signer_pubkey: signature.map(|(_, pk)| to_hex(&pk)),
        meta: reader.meta().to_vec(),
    };
    store.publish_asset(&record)?;
    Ok(stats)
}
