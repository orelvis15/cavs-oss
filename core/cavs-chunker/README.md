# cavs-chunker

Chunking strategies: how content is split into chunks before hashing.

## What it does

- **Fixed** — fixed-size chunks aligned to the start (stable, CDN-friendly for
  already-packaged media segments).
- **FastCDC** — content-defined chunking that resists insertions and
  reordering: unchanged regions produce identical chunks even after a shift.

Presets: `media_default()` (256 KiB fixed), `asset_default()` (FastCDC
16/64/256 KiB — the game-asset default), `screen_default()` (aggressive CDC).

## Use

```rust
use cavs_chunker::{split, ChunkMode};
for range in split(&data, ChunkMode::asset_default()) {
    let chunk = &data[range];
    // hash / store chunk
}
```
