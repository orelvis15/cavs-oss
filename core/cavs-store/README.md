# cavs-store

Content-addressable storage for CAVS.

## What it does

- `CasIndex` — an in-memory hash→index map with reference counts, used while
  packing to deduplicate chunks.
- `GlobalStore` — an on-disk global content-addressable store: every unique
  chunk is stored once (`chunks/<ab>/<hex>`) across all assets and versions,
  with an `index.json` reference-count ledger and per-asset records. Supports
  `publish` / `unpublish` and garbage collection of zero-reference chunks after
  a grace period.

This is what turns per-file egress dedup into real **at-rest storage dedup** on
the origin. Driven by `cavs store …` and `cavs-server --store`.

## Use

```rust
use cavs_store::GlobalStore;
let mut store = GlobalStore::open("./store".as_ref())?;
store.put_chunk(&hash, &stored_bytes, flags, len_raw)?;
store.publish_asset(&record)?;   // increments refcounts
let (removed, bytes) = store.gc(0)?;   // reclaim unreferenced chunks
```
