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
```

## Options

- `--cache <dir>` — persistent chunk cache (default `.cavs-cache`); safe to
  delete, shared across versions and assets.
- `--pubkey <hex|file>` — require the asset to be signed by this Ed25519 key.
- `--ca <pem>` — trust a specific certificate (e.g. a self-signed dev cert).
- `--stats-json <path>` — write exact fetch statistics (inline bytes, refs, …).

Run `cavs-client --help` for all options.
