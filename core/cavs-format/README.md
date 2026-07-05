# cavs-format

The `.cavs` binary container: types, streaming writer, and hardened reader.

## What it does

- **Writer** — streams chunks into the DATA section as they arrive
  (deduplicated), then writes the tables and an optional Ed25519 content
  signature. Only the small tables are buffered.
- **Reader / verifier** — reads chunks (raw or decompressed + BLAKE3-verified),
  checks section hashes and the Merkle root, and verifies the content
  signature. Every offset/length is validated against the real file size and
  allocations are bounded, so a malformed or adversarial `.cavs` errors out
  instead of panicking or exhausting memory.

The byte-level specification is in [`../../docs/FORMAT.md`](../../docs/FORMAT.md).

## Use

```rust
use cavs_format::{Writer, Reader};
let mut w = Writer::create(path, uuid, 1000, /*compress*/ true)?;
let idx = w.add_chunk(bytes)?;
w.finish()?;

let mut r = Reader::open(path)?;
r.verify()?;                       // full integrity check
let bytes = r.read_chunk(idx)?;    // decompressed + verified
```
