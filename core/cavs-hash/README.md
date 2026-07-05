# cavs-hash

Hashing primitives shared across CAVS.

## What it does

- `hash_chunk(bytes)` — BLAKE3-256, the content identity of a chunk.
- Incremental `Hasher` for streaming section hashes.
- `merkle_root(hashes)` — binary Merkle root over the chunk table (odd nodes
  promoted; empty list = `blake3("")`).
- `content_signature_message(root, count)` — the canonical message signed by
  Ed25519 content signatures (`"CAVS1-SIG-V1" || merkle_root || chunk_count`).
- `to_hex` / `from_hex` helpers.

A pinned interoperability test vector (Merkle root over fixed inputs) lives in
this crate's tests so third-party decoders can validate their implementation.

## Use

```rust
use cavs_hash::{hash_chunk, merkle_root, to_hex};
let h = hash_chunk(b"chunk bytes");
let root = merkle_root(&[h]);
println!("{}", to_hex(&root));
```
