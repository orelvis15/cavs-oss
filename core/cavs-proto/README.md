# cavs-proto

The CVSP wire protocol types shared by server and clients.

## What it does

- **Control plane**: `Manifest`, `SessionOpenRequest` /
  `SessionOpenResponse`, `BatchRequest`, `AssetSummary`. Sessions travel as
  JSON; the manifest travels as JSON v1 or as the compact binary v2 format
  implemented in the `cavs-manifest` crate (both decode into the same
  `Manifest`).
- **Data plane (binary `CVSP`)**: compact batch encoding of delivery
  instructions — `Ref` (client already has the chunk) or `Inline` (payload, as
  stored, possibly zstd). `decode_stream` consumes a batch incrementally from a
  reader so the client's peak memory is one chunk.
- **`BloomFilter`** — a compact summary of a client's have-set so session-open
  stays small even with tens of thousands of cached chunks; false positives are
  repaired by fetching the chunk directly by hash.
- **`errors`** (v0.5.0) — the stable `CAVS-E-*` error taxonomy shared by the
  CLI, server and client (`CAVS-E-MANIFEST-CORRUPT`,
  `CAVS-E-CHUNK-HASH-MISMATCH`, `CAVS-E-NETWORK`, …), recoverable from any
  rendered error chain with `error_code_of`.

The batch decoders are hardened (v0.5.0): item counts are never
pre-allocated beyond what the buffer could encode, and inline chunk lengths
are validated against a 256 MiB ceiling (`MAX_WIRE_CHUNK`) before any
allocation — fuzzed under `fuzz/` and replayed deterministically in CI.

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
