//! Hashing primitives for CAVS-1: BLAKE3-256 chunk identity and Merkle roots
//! for incremental / whole-file verification.

/// 256-bit content hash. Identity of a chunk is `blake3(raw_bytes)` over the
/// *uncompressed* payload, so identity is stable regardless of storage
/// compression.
pub type ChunkHash = [u8; 32];

/// Hash algorithm identifiers as stored in the superblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HashAlgo {
    Blake3 = 1,
}

impl HashAlgo {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(HashAlgo::Blake3),
            _ => None,
        }
    }
}

/// Hash a chunk payload (uncompressed bytes).
pub fn hash_chunk(data: &[u8]) -> ChunkHash {
    *blake3::hash(data).as_bytes()
}

/// Incremental BLAKE3 hasher, for hashing streamed sections without
/// buffering them in memory.
#[derive(Default)]
pub struct Hasher(blake3::Hasher);

impl Hasher {
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(&self) -> ChunkHash {
        *self.0.finalize().as_bytes()
    }
}

/// Hex-encode a hash for display.
pub fn to_hex(hash: &ChunkHash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse a hex string back into a hash.
pub fn from_hex(s: &str) -> Option<ChunkHash> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Canonical message signed by CAVS-1 content signatures (Ed25519):
/// domain tag || Merkle root over the chunk table || chunk count.
/// Covers every content byte; table/segment structure is protected by the
/// per-section hashes verified locally and by TLS in transit.
pub fn content_signature_message(merkle_root: &ChunkHash, chunk_count: u64) -> Vec<u8> {
    let mut msg = Vec::with_capacity(12 + 32 + 8);
    msg.extend_from_slice(b"CAVS1-SIG-V1");
    msg.extend_from_slice(merkle_root);
    msg.extend_from_slice(&chunk_count.to_le_bytes());
    msg
}

/// Binary Merkle root over an ordered list of chunk hashes.
///
/// Leaves are the chunk hashes themselves; internal nodes are
/// `blake3(left || right)`. An odd node at any level is promoted unchanged.
/// The root of an empty list is `blake3("")` so it is always defined.
pub fn merkle_root(hashes: &[ChunkHash]) -> ChunkHash {
    if hashes.is_empty() {
        return *blake3::hash(b"").as_bytes();
    }
    let mut level: Vec<ChunkHash> = hashes.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            if pair.len() == 2 {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&pair[0]);
                buf[32..].copy_from_slice(&pair[1]);
                next.push(*blake3::hash(&buf).as_bytes());
            } else {
                next.push(pair[0]);
            }
        }
        level = next;
    }
    level[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_hash_is_deterministic() {
        assert_eq!(hash_chunk(b"hello"), hash_chunk(b"hello"));
        assert_ne!(hash_chunk(b"hello"), hash_chunk(b"hellp"));
    }

    #[test]
    fn hex_roundtrip() {
        let h = hash_chunk(b"roundtrip");
        assert_eq!(from_hex(&to_hex(&h)), Some(h));
    }

    #[test]
    fn merkle_single_leaf_is_leaf() {
        let h = hash_chunk(b"leaf");
        assert_eq!(merkle_root(&[h]), h);
    }

    #[test]
    fn merkle_changes_with_any_leaf() {
        let a = hash_chunk(b"a");
        let b = hash_chunk(b"b");
        let c = hash_chunk(b"c");
        let r1 = merkle_root(&[a, b, c]);
        let r2 = merkle_root(&[a, b, hash_chunk(b"c!")]);
        assert_ne!(r1, r2);
        // Order matters.
        assert_ne!(merkle_root(&[a, b, c]), merkle_root(&[c, b, a]));
    }

    #[test]
    fn merkle_empty_is_defined() {
        assert_eq!(merkle_root(&[]), merkle_root(&[]));
    }

    /// Interop anchor: pins the chunk-hash + Merkle definitions so any
    /// third-party decoder can validate against a known vector. If this
    /// changes, the on-wire format has changed — bump the format version.
    #[test]
    fn format_test_vector() {
        let leaves: Vec<ChunkHash> = (0..5u32)
            .map(|i| hash_chunk(format!("cavs-vector-{i}").as_bytes()))
            .collect();
        assert_eq!(
            to_hex(&merkle_root(&leaves)),
            "1ea34f80f682f6cb859845f2d26c52f9f3b7052be7bdf641844e19b01d3d329e"
        );
        // And a single known chunk hash.
        assert_eq!(
            to_hex(&hash_chunk(b"cavs-vector-0")),
            to_hex(&leaves[0])
        );
    }
}
