# CAVS — Content-Addressable Verified Streaming

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
| MechanicalFlower/**Marble** | 1.6.0 → 1.6.1 | 6.55 MiB | 0.19 MiB | **−97.1%** |
| GDQuest **3D third-person** | HEAD~10 → HEAD (468 files) | 27.61 MiB | 8.7 MiB | **−68%** |

- **Re-downloads cost ~0 bytes** of payload (persistent content-addressable cache).
- **Server storage dedup**: ingesting two versions of a real game into the
  global store stored the shared chunks once — **~49% less disk** than keeping
  each `.cavs` separately.
- **Client RAM is constant at ~7 MiB**, whether the game is 9 MB or 569 MB.
- **Honest negatives**: on a single video, ABR ladders, or already-compressed
  files, savings are ~0 and the packaging overhead is +0.03–2%.

Full methodology and comparisons vs xdelta3/bsdiff/rdiff/rsync are in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md). Design rationale and results are in
the paper, [`docs/PAPER.md`](docs/PAPER.md).

## Repository layout

| Folder | What |
|---|---|
| [`core/`](core) | The delivery engine (Rust): chunking, hashing, the `.cavs` format, the global content-addressable store, the CVSP protocol, and the `cavs` / `cavs-server` / `cavs-client` binaries |
| [`steam-analyzer/`](steam-analyzer) | `cavs-steam` — estimates the SteamPipe update size of a build and flags pack files that cause update bloat, before you publish to Steam |
| [`godot-plugin/`](godot-plugin) | Godot 4 runtime client in pure GDScript: downloads, verifies and mounts packs with `load_resource_pack()` |
| [`docs/`](docs) | Format specification, architecture, benchmarks, and the technical paper |

## Quick start

```sh
cargo build --release

# 1. Package two versions of a game build
./target/release/cavs pack --raw game_v1.pck -o game_v1.cavs
./target/release/cavs pack --raw game_v2.pck -o game_v2.cavs

# 2. Serve them
./target/release/cavs-server game_v1.cavs game_v2.cavs --listen 127.0.0.1:8990

# 3. A client installs v1, then updates to v2 downloading only what changed
./target/release/cavs-client fetch http://127.0.0.1:8990 game_v1 -o out1 --cache ./cache
./target/release/cavs-client fetch http://127.0.0.1:8990 game_v2 -o out2 --cache ./cache
#                                                          ↑ this second fetch is a fraction of the size
```

Global content-addressable store (dedup at rest across all versions):

```sh
./target/release/cavs store ./store add game_v1 game_v1.cavs
./target/release/cavs store ./store add game_v2 game_v2.cavs   # shared chunks stored once
./target/release/cavs store ./store stat                        # storage savings
./target/release/cavs-server --store ./store --listen 127.0.0.1:8990
```

## Components

- **`cavs`** (CLI): package files/builds into `.cavs` (FastCDC + zstd +
  optional Ed25519 signature), inspect, verify, reconstruct, and manage a
  global store (`add` / `rm` / `gc` / `stat`).
- **`cavs-server`**: stateful HTTP/HTTPS origin. Per-session have-set,
  inline/reference planning, CVSP binary batches, immutable CDN-cacheable
  chunk endpoint, Prometheus metrics, and a `--store` mode.
- **`cavs-client`**: native streaming client with a persistent cache and
  atomic, verified reconstruction; resumable and retry-safe.
- **Godot plugin**: `CavsClient` in pure GDScript (no native binaries) —
  install as an addon, mount packs at runtime. See
  [`godot-plugin/README.md`](godot-plugin/README.md).
- **SteamPipe Analyzer**: see [`steam-analyzer/README.md`](steam-analyzer/README.md).

## Requirements

- Rust (stable).
- `ffmpeg` on `PATH` only for the optional video packaging mode; the `--raw`
  (game asset) mode needs nothing extra.

## License

See [`LICENSE`](LICENSE). This is currently a source-available / evaluation
license: reading, building and reproducing the benchmarks are free; production
use requires a commercial agreement. Choose your final open-source license
before publishing if you intend a permissive model.
