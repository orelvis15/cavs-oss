# CAVS — Content-Addressable Verified Streaming

[![CI](https://github.com/orelvis15/cavs-oss/actions/workflows/ci.yml/badge.svg)](https://github.com/orelvis15/cavs-oss/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

**Ship game updates that weigh what *changed*, not what the game weighs.**

CAVS is a content-addressable, verified delivery layer for **game content** —
builds, Godot PCK files, AssetBundles, binary bundles, patches. It sits **on
top of** your existing formats (it doesn't replace them) and makes every
client download only the chunks it doesn't already have. It also packages
video (HLS/CMAF segments), but game asset delivery is the focus — CAVS is not
a pixel codec.

- **Content-addressable**: every chunk is identified by its BLAKE3-256 hash;
  the client fetches only what it lacks, with a cache that reuses bytes across
  versions, DLC and sessions.
- **Cold installs at less than full-download price (dual route)**: packing
  with `--bootstrap` also emits the whole release as one zstd-19 artifact;
  the server routes cache-less clients to it automatically whenever it beats
  the chunk path, and the client **seeds its chunk cache from it** — so the
  first install is cheap *and* the next update is already incremental.
- **Adaptive chunking**: `--profile auto` classifies the payload (format
  magic, sampled entropy, compression probe) and measures candidate chunk
  profiles on the real bytes; `cavs sweep` prints the per-title table.
- **Verified end-to-end**: per-chunk BLAKE3, global Merkle root, per-file
  SHA-256, and an optional Ed25519 content signature. Reconstruction is
  byte-identical or it fails — never halfway.
- **Constant memory**: the client reconstructs by streaming to disk
  (`.part` → verify → atomic rename), so RAM stays ~constant regardless of
  game size.
- **Complementary, not competitive**: use the best codec/compressor for the
  bytes; CAVS deduplicates and transports above them.

## Why it matters (measured on real games)

Two real versions of open-source Godot games were exported to PCK and served
over real HTTP sessions. "Update" = what a player who already has the previous
version downloads, versus downloading the full new release compressed with zstd.

| Game | Update | Full download | With CAVS | Saved |
|---|---|---:|---:|---:|
| godotengine/**tps-demo** (569 MB) | tag 4.5 → master | 247.6 MiB | **1.64 MiB** | **−99.3%** |
| MechanicalFlower/**Marble** | 1.6.0 → 1.6.1 | 6.55 MiB | 0.14 MiB | **−97.8%** |
| GDQuest **3D third-person** | HEAD~10 → HEAD (468 files) | 27.61 MiB | 8.7 MiB | **−68.5%** |

And the **first install** (a cache-less player) now costs *less* than
downloading the full compressed release, thanks to the dual delivery route:

| Game | Full download (zstd-3) | CAVS cold install | Delta |
|---|---:|---:|---:|
| godotengine/**tps-demo** | 247.62 MiB | **221.42 MiB** | **−10.6%** |
| GDQuest **3D third-person** | 27.66 MiB | **24.43 MiB** | **−11.7%** |
| MechanicalFlower/**Marble** | 6.55 MiB | **5.68 MiB** | **−13.2%** |

- **Re-downloads cost ~0 bytes** of payload (persistent content-addressable cache).
- **Server storage dedup**: ingesting two versions of a real game into the
  global store stored the shared chunks once — **~49% less disk** than keeping
  each `.cavs` separately.
- **Client RAM is constant at ~7 MiB**, whether the game is 9 MB or 569 MB.
- **Honest negatives**: on a single video, ABR ladders, or already-compressed
  files, savings are ~0 and the packaging overhead is +0.03–2% (the payload
  classifier keeps it at the low end by using large chunks there).

Full methodology and comparisons vs xdelta3/bsdiff/rdiff/rsync are in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md). Design rationale and results are in
the paper, [`docs/PAPER.md`](docs/PAPER.md).

## Repository layout

| Folder | What |
|---|---|
| [`core/`](core) | The delivery engine (Rust): chunking, hashing, the `.cavs` format, the global content-addressable store, the CVSP protocol, and the `cavs` / `cavs-server` / `cavs-client` binaries |
| [`steam-analyzer/`](steam-analyzer) | `cavs-steam` — estimates the SteamPipe update size of a build and flags pack files that cause update bloat, before you publish to Steam |
| [`godot-plugin/`](godot-plugin) | Godot 4 runtime client in pure GDScript: downloads, verifies and mounts packs with `load_resource_pack()` |
| [`unity-plugin/`](unity-plugin) | Unity package — **coming soon** |
| [`unreal-plugin/`](unreal-plugin) | Unreal Engine plugin — **coming soon** |
| [`docs/`](docs) | Format specification, architecture, benchmarks, and the technical paper |

## Getting started

### Prerequisites

- **Rust** (stable) — install via [rustup](https://rustup.rs). No other
  dependency is needed for the game-asset (`--raw`) path.
- **ffmpeg** on `PATH` — only for the optional video packaging mode.
- **Godot 4** — only if you use the Godot plugin.

### Build

```sh
git clone https://github.com/orelvis15/cavs-oss.git && cd cavs-oss
cargo build --release
```

This produces the binaries in `target/release/`:

- `cavs` — the packaging CLI
- `cavs-server` — the origin server
- `cavs-client` — the native client
- `cavs-steam` — the SteamPipe analyzer

### Test

```sh
cargo test            # unit + integration + end-to-end tests
cargo clippy --all-targets   # lints
```

### Try it end to end

Package two versions of a build, serve them, and watch a client download only
what changed on the second fetch:

```sh
# 1. Package two versions of a game build. --profile auto picks the chunking
#    per payload, --bootstrap makes cold installs cost the full artifact, and
#    --prev keeps the chunk profile consistent with the published version.
./target/release/cavs pack --raw game_v1.pck --profile auto --bootstrap -o game_v1.cavs
./target/release/cavs pack --raw game_v2.pck --profile auto --prev game_v1.cavs --bootstrap -o game_v2.cavs

# 2. Inspect and verify
./target/release/cavs info game_v1.cavs
./target/release/cavs verify game_v1.cavs

# 3. Serve both versions (the .bootstrap.zst sidecars are picked up next to them)
./target/release/cavs-server game_v1.cavs game_v2.cavs --listen 127.0.0.1:8990

# 4. A cold client installs v1 (routed to the bootstrap, cache auto-seeded),
#    then updates to v2 — the second fetch downloads only the changed chunks
./target/release/cavs-client fetch http://127.0.0.1:8990 game_v1 -o out1 --cache ./cache
./target/release/cavs-client fetch http://127.0.0.1:8990 game_v2 -o out2 --cache ./cache

# Optional: measure which chunk profile is cheapest for YOUR builds
./target/release/cavs sweep game_v2.pck --prev game_v1.cavs
```

Signing (optional, recommended for distribution):

```sh
./target/release/cavs keygen -o publisher.key                     # → publisher.key(.pub)
./target/release/cavs pack --raw game_v2.pck --sign-key publisher.key -o game_v2.cavs
./target/release/cavs-client fetch <url> game_v2 -o out --cache ./cache --pubkey publisher.key.pub
```

### Global content-addressable store (dedup at rest across all versions)

Store each unique chunk once across every version/title, with reference
counting and garbage collection:

```sh
./target/release/cavs store ./store add game_v1 game_v1.cavs
./target/release/cavs store ./store add game_v2 game_v2.cavs   # shared chunks stored once
./target/release/cavs store ./store stat                        # storage savings
./target/release/cavs store ./store gc --grace 0                # reclaim unreferenced chunks
./target/release/cavs-server --store ./store --listen 127.0.0.1:8990
```

### Analyze a Steam build

```sh
./target/release/cavs-steam compare ./build_v1 ./build_v2 --out report
open report/index.html
```

See [`godot-plugin/README.md`](godot-plugin/README.md) for game integration and
[`steam-analyzer/README.md`](steam-analyzer/README.md) for the analyzer.

## Components

- **`cavs`** (CLI): package files/builds into `.cavs` (FastCDC + zstd +
  optional Ed25519 signature) with payload classification, `--profile auto`
  chunk-profile selection, `--bootstrap` cold-install artifacts and a
  `sweep` cost report; inspect, verify, reconstruct, and manage a global
  store (`add` / `rm` / `gc` / `stat`).
- **`cavs-server`**: stateful HTTP/HTTPS origin. Per-session have-set,
  inline/reference planning, dual-route decision (bootstrap vs chunks) per
  client, CVSP binary batches, immutable CDN-cacheable chunk endpoint,
  Prometheus metrics, and a `--store` mode.
- **`cavs-client`**: native streaming client with a persistent cache and
  atomic, verified reconstruction; takes the bootstrap route when offered
  (seeding its cache); resumable and retry-safe.
- **Godot plugin**: `CavsClient` in pure GDScript (no native binaries) —
  install as an addon, mount packs at runtime. See
  [`godot-plugin/README.md`](godot-plugin/README.md).
- **SteamPipe Analyzer**: see [`steam-analyzer/README.md`](steam-analyzer/README.md).

## Contributing

Contributions are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md) for setup,
the PR workflow, and the checklist. Every PR runs CI (format, clippy, tests).
Releases are cut by the maintainer by pushing a version tag (`v*`), which
triggers the [release workflow](.github/workflows/release.yml) to build and
publish versioned binaries for Linux, macOS and Windows.

## License

Licensed under the **Apache License, Version 2.0** — see [`LICENSE`](LICENSE)
and [`NOTICE`](NOTICE). You may use, modify and distribute this software freely
under its terms; it includes an express patent grant.
