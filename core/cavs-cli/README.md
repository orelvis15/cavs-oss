# cavs — packaging CLI

The `cavs` binary turns files and game builds into deduplicated, verifiable
`.cavs` files, inspects and verifies them, reconstructs them byte-for-byte, and
manages the global content-addressable store.

## How it works

`cavs pack --raw` splits inputs into FastCDC chunks (~64 KiB), deduplicates
them, compresses each with zstd, and writes a `.cavs` container with a chunk
table, a Merkle root over the content, per-file SHA-256, and an optional
Ed25519 signature. Reconstruction (`unpack`) verifies every chunk and produces
the original files exactly. The video mode (without `--raw`) segments inputs
with ffmpeg first, then packages the segments.

With `--profile auto` the packer first **classifies** each input (format
magic, sampled entropy, a zstd probe) and **measures** candidate chunk
profiles on the real bytes, picking the cheapest by a weighted cost model:
already-compressed payloads get large fixed chunks, engine packs get CDC.
`--bootstrap` additionally writes `<output>.bootstrap.zst` — the whole input
compressed at zstd-19 — which the server offers to cache-less clients so a
first install costs the full artifact (and seeds the client's cache). When
packing the next version, pass `--prev <published .cavs>` so profile choice
stays consistent with the chunks clients already have.

## Commands

```sh
# Package (game assets / arbitrary files)
cavs pack --raw build_v42.pck -o v42.cavs
cavs pack --raw build_v42.pck --profile auto --bootstrap -o v42.cavs  # v2 pipeline
cavs pack --raw build_v43.pck --profile auto --prev v42.cavs --bootstrap -o v43.cavs
cavs pack --raw --sign-key publisher.key data/* -o release.cavs   # signed
cavs pack --raw --mode screen capture.bin -o capture.cavs         # aggressive CDC

# Measure candidate chunk profiles on your own builds (optionally vs the
# published previous version, to see real chunk reuse)
cavs sweep build_v43.pck --prev v42.cavs --json sweep.json

# Package video (needs ffmpeg on PATH)
cavs pack movie.mp4 -o movie.cavs --segment-time 4

# Inspect / verify / reconstruct
cavs info v42.cavs                       # structure, dedupe, compression
cavs verify v42.cavs --pubkey key.pub    # chunk hashes + Merkle + signature
cavs unpack v42.cavs -o restored/        # exact reconstruction
cavs play movie.cavs                     # reconstruct to temp and play (ffplay)

# Signing keys (Ed25519)
cavs keygen -o publisher.key             # → publisher.key (+ .pub)

# Global store (dedup at rest across versions/titles)
cavs store ./store add game_v1 game_v1.cavs
cavs store ./store add game_v2 game_v2.cavs   # shared chunks stored once
cavs store ./store stat                        # storage savings
cavs store ./store rm  game_v1                 # unpublish
cavs store ./store gc  --grace 0               # reclaim unreferenced chunks
```

Run `cavs --help` or `cavs <command> --help` for all options.

## Requirements

Nothing extra for `--raw` (game asset) mode. `ffmpeg`/`ffplay` on `PATH` only
for the video mode.
