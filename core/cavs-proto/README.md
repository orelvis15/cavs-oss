# cavs-proto

The CVSP wire protocol types shared by server and clients.

## What it does

- **Control plane (JSON)**: `Manifest`, `SessionOpenRequest` /
  `SessionOpenResponse`, `BatchRequest`, `AssetSummary`.
- **Data plane (binary `CVSP`)**: compact batch encoding of delivery
  instructions — `Ref` (client already has the chunk) or `Inline` (payload, as
  stored, possibly zstd). `decode_stream` consumes a batch incrementally from a
  reader so the client's peak memory is one chunk.
- **`BloomFilter`** — a compact summary of a client's have-set so session-open
  stays small even with tens of thousands of cached chunks; false positives are
  repaired by fetching the chunk directly by hash.

## Use

```rust
use cavs_proto::{BatchResponse, decode_stream, BatchItem};
// server: build and encode a batch
let wire = batch.encode();
// client: decode incrementally
decode_stream(&mut reader, |item| {
    if let BatchItem::Instr(instr) = item { /* handle ref/inline */ }
    Ok(())
})?;
```
