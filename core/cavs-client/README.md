# cavs-client — native client

The `cavs-client` binary fetches an asset from a `cavs-server`, downloading only
the chunks it doesn't already have, and reconstructs the original files.

## How it works

It keeps a **persistent content-addressable cache** on disk. On fetch it
announces its have-set, receives a stream of inline/reference instructions,
decompresses and BLAKE3-verifies each inline chunk into the cache, then
reconstructs each output file by streaming chunks from the cache into a `.part`
temp file, verifying the manifest's SHA-256, and atomically renaming into
place. Peak memory is one chunk regardless of asset size; interrupted fetches
resume without re-downloading, and a failed verification never leaves a corrupt
file.

**Dual route (v2)**: when the server measures that the full bootstrap
artifact is cheaper for this cache (typically a first install), the client
downloads it instead — streamed to disk, BLAKE3-verified on the wire,
SHA-256-verified after decompression, installed atomically — and then **seeds
its chunk cache** by slicing the installed file along the manifest's chunk
plan. The next update therefore only pays for changed chunks. Any failure on
this path falls back to the normal chunk route. `--stats-json` reports which
route was taken (`delivery_mode`), plus `seeded_chunks` and `seed_ms`.

**Compact manifest (v0.3.0)**: the client requests the binary v2 manifest
(~76% smaller than JSON) and falls back to JSON v1 transparently on older
servers — the format is detected from the bytes. `--stats-json` includes a
`manifest` block: `format`, `wire_bytes`, `parse_ms`, `chunk_count_logical`,
`chunk_count_unique`.

**Hardening (v0.5.0)**: a crash-safe journal under `<cache>/journal/`
records every in-flight fetch. An interrupted bootstrap download keeps its
`.zst.part` and continues with an HTTP `Range` request on the next fetch
(or `cavs-client resume`); interrupted chunk fetches resume from the cache
have-set. Transient network failures (transport errors, 429/5xx) retry
with exponential backoff (250 ms → 8 s, jittered, 5 attempts); hash
mismatches never retry. The cache maintains itself: `cache verify`
quarantines corrupt entries, `cache repair` re-fetches exactly what an
asset is missing, `cache gc` evicts LRU to a size budget. Failures carry
stable `CAVS-E-*` codes (`CAVS-E-BOOTSTRAP-HASH-MISMATCH`,
`CAVS-E-CACHE-CORRUPT-RECOVERABLE`, `CAVS-E-NETWORK`, …).

## Use

```sh
# List assets on a server
cavs-client list http://127.0.0.1:8990

# Fetch (first time installs everything; later versions download only changes)
cavs-client fetch http://127.0.0.1:8990 game_v1 -o out1 --cache ./cache
cavs-client fetch http://127.0.0.1:8990 game_v2 -o out2 --cache ./cache

# Require a trusted signer, and export exact stats
cavs-client fetch <url> game_v2 -o out --cache ./cache \
  --pubkey publisher.key.pub --stats-json stats.json

# Play a fetched video asset (needs ffplay)
cavs-client play <url> movie --cache ./cache

# Resume interrupted fetches (v0.5.0; fetch also resumes by default)
cavs-client resume --cache ./cache

# Cache maintenance (v0.5.0)
cavs-client cache verify --cache ./cache                  # quarantine rot
cavs-client cache repair <url> game_v2 --cache ./cache    # re-fetch missing
cavs-client cache gc --cache ./cache --max-size 10GiB     # LRU eviction
```

## Options

- `--cache <dir>` — persistent chunk cache (default `.cavs-cache`); safe to
  delete, shared across versions and assets.
- `--pubkey <hex|file>` — require the asset to be signed by this Ed25519 key.
- `--ca <pem>` — trust a specific certificate (e.g. a self-signed dev cert).
- `--stats-json <path>` — write exact fetch statistics (inline bytes, refs, …).
- `--no-resume` — start clean instead of resuming an interrupted fetch.

Run `cavs-client --help` for all options.
