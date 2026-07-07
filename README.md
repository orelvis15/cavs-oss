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
- **Compact manifests (v0.3.0)**: the runtime manifest travels as a compact
  binary format (`CAVSMF2`) — ~75–77% smaller than the JSON equivalent on real
  games — negotiated transparently, with JSON kept as the default response,
  debug export and compatibility path for older clients.
- **Packfile storage (v0.4.0)**: the global store can keep its chunks in a
  few immutable, content-addressed `.cavspack` files instead of one file per
  chunk — on a 570 MB game, 5,775 objects become 6 files and an update
  session's reads coalesce 170× with zero read amplification. Exportable as
  a deterministic object tree for S3/R2/CDN.
- **Production-hardened (v0.5.0)**: interrupted downloads resume with HTTP
  Range instead of restarting; transient network failures retry with
  backoff; a corrupt cache is detected, quarantined and repaired in place;
  every decoder is fuzzed and survives full byte-flip/truncation sweeps;
  failures carry stable `CAVS-E-*` error codes; and `cavs doctor` diagnoses
  a deployment in one command.
- **Offline toolkit (v0.7.0)**: sign, preview, diff, apply, verify and
  benchmark builds locally with no server — `cavs preview` /`diff-plan` /
  `apply` / `verify-install` / `file` / `ls`, portable `.cavsplan` patches
  with journaled staged applies, stable directory mode with `.cavsignore`,
  and a fair external **butler offline** benchmark plus a multi-route
  comparison suite.
- **Delivery planner (v0.8.0)**: `cavs route-plan` scores every route
  (no-op / chunks / hybrid / plan / sidecar / bootstrap / full) for one
  concrete client state under device profiles; `.cavspatch` v2 optimized
  sidecars pick the best strategy **per file** (copy-old with rename
  detection, plan ops, bsdiff, xdelta3, full data) by measuring real
  candidates; `cavs patch-policy` keeps sidecars to hot pairs (never
  O(N²)); `cavs publish-dir` produces a whole release in one pass; and
  `cavs bench full-pipeline` proves it against the complete external
  butler pipeline (default *and* rediff-optimized patches).
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
- **Interrupted installs don't start over** (v0.5.0): a 232 MiB install
  killed at 57 MiB resumed with an HTTP Range request and downloaded only
  the missing ~166 MiB — verified end to end, never promoting an
  unverified file.
- **Content shifts don't break updates**: inserting bytes at the head of a
  1 GiB build (every downstream byte moves) costs **10.9 KiB** of update
  egress — FastCDC re-synchronizes (`cavs bench suite`, reproducible).
- **Server storage dedup**: ingesting two versions of a real game into the
  global store stored the shared chunks once — **~49% less disk** than keeping
  each `.cavs` separately.
- **Client RAM is constant at ~7–14 MiB**, whether the game is 9 MB or 569 MB.
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
counting and garbage collection. With `--storage packfiles` (v0.4.0) the
chunks live in a few immutable packfiles served by coalesced range reads:

```sh
./target/release/cavs store ./store add game_v1 game_v1.cavs --storage packfiles
./target/release/cavs store ./store add game_v2 game_v2.cavs   # shared chunks stored once
./target/release/cavs store ./store stat                        # storage savings + pack occupancy
./target/release/cavs store ./store verify                      # re-hash chunks, check packs
./target/release/cavs store ./store gc --grace 0                # reclaim unreferenced chunks/packs
./target/release/cavs store ./store export --out ./dist         # immutable tree for S3/R2/CDN
./target/release/cavs-server --store ./store --listen 127.0.0.1:8990
```

### Operate it in production (v0.5.0)

Interrupted downloads resume by default; the cache heals itself; one command
diagnoses a deployment:

```sh
# Resume whatever fetches were interrupted (bootstrap downloads continue
# via HTTP Range; chunk fetches continue from the cache have-set)
./target/release/cavs-client resume --cache ./cache

# Cache maintenance: re-hash everything (corrupt entries -> quarantine),
# re-fetch an asset's missing/corrupt chunks, evict LRU to a size budget
./target/release/cavs-client cache verify --cache ./cache
./target/release/cavs-client cache repair http://127.0.0.1:8990 game_v2 --cache ./cache
./target/release/cavs-client cache gc --cache ./cache --max-size 10GiB

# Diagnose: container integrity, manifest, bootstrap sidecar, store, cache
./target/release/cavs doctor game_v2.cavs --store ./store --cache ./cache

# Prove every decoder rejects corruption cleanly (20-row mutation matrix)
./target/release/cavs test corrupt game_v2.cavs --out corrupt-report.json

# Reproducible large-build benchmarks (deterministic synthetic datasets)
./target/release/cavs bench gen --out ./ds --size 1GiB
./target/release/cavs bench suite --dataset ./ds --out ./results
```

Failures carry stable error codes (`CAVS-E-BOOTSTRAP-HASH-MISMATCH`,
`CAVS-E-CACHE-CORRUPT-RECOVERABLE`, `CAVS-E-NETWORK`, …) so launchers and
scripts can decide retry/repair/give-up without parsing prose.

### Hybrid reconstruction (v0.6.0)

The previous installed version is now a first-class byte source: a client
with an **empty cache but the old build on disk** copies verified ranges
from it and downloads only what changed (measured: −90.3 % wire on a small
update vs v0.5's cold path, −99.98 % on a shifted build — see
[docs/HYBRID_RECONSTRUCTION.md](docs/HYBRID_RECONSTRUCTION.md)):

```sh
# Update reusing the old install directly (works with a cold cache)
./target/release/cavs-client fetch http://127.0.0.1:8990 game_v2 \
  -o ./install --cache ./cache --previous-artifact ./install/game_v1.pck

# Compact old-version signatures (~0.07% of the source)
./target/release/cavs signature export game_v1.cavs -o game_v1.cavssig
./target/release/cavs pack --raw game_v2.pck --against-signature game_v1.cavssig -o v2.cavs

# Directory/container mode (preview): per-file dedup, staged installs,
# unchanged (modded) files untouched
./target/release/cavs pack-dir ./Build_v2 -o build_v2.cavs
./target/release/cavs-client fetch http://127.0.0.1:8990 build_v2 -o ./InstalledGame --cache ./cache

# Compare against a block-based delta patcher (and xdelta3/bsdiff if present)
./target/release/cavs bench delta --old game_v1.pck --new game_v2.pck --out results/delta
```

Already-current outputs are detected and skipped (no-op: 0 bytes), every
copied range is BLAKE3-verified before it is written, and a corrupt old
install demotes to cache/network instead of failing.

### Offline toolkit (v0.7.0)

Sign, preview, diff, apply and verify updates locally — no CAVS server. The
offline apply uses the same verified reconstruction model as the online
client, so a `.cavsplan` update is byte-identical or it fails:

```sh
# 1. Describe the released version once (compact, ~0.07% of the source)
./target/release/cavs signature export ./Build_v1 --raw -o build_v1.cavssig

# 2. See what the next build changes before publishing anything
./target/release/cavs preview ./Build_v2 --against build_v1.cavssig --changes-only

# 3. Produce a deterministic offline update plan (a portable patch)
./target/release/cavs diff-plan ./Build_v1 ./Build_v2 -o update.cavsplan --report plan.md

# 4. Apply it in place — staged, journaled, verified, mod-friendly
./target/release/cavs apply --old ./InstalledGame --plan update.cavsplan --inplace --verify

# 5. Check any install against a known-good signature (mods tolerated)
./target/release/cavs verify-install ./InstalledGame --signature build_v2.cavssig --allow-extra-files

# Identify/inspect any CAVS file
./target/release/cavs file update.cavsplan
./target/release/cavs ls build_v1.cavssig
```

Directory builds are first-class: `cavs pack-dir ./Build -o b.cavs --ignore
'*.pdb' --ignore 'logs/'` (also reads a root `.cavsignore`). Measured on a
128 MiB build, the offline `.cavsplan` update is **2.51 MiB** (directory) and
**1.94 MiB** (single artifact) — matching butler's offline patch while
applying with a streaming ~8 MiB memory budget. Benchmark it yourself:

```sh
# Every delivery route for one transition (butler + pairwise proxies optional)
./target/release/cavs bench routes --old ./Build_v1 --new ./Build_v2 \
  --butler-bin ./butler --include-pairwise-proxy --out results/routes

# Fair external butler offline diff/apply/verify harness
./target/release/cavs bench butler-offline --old ./Build_v1 --new ./Build_v2 \
  --butler-bin ./butler --out results/butler

# Many-version storage: store-once vs per-pair patches
./target/release/cavs bench version-stream --out results/stream --versions 10
```

The butler harness measures butler's **offline/default** patch, not the
backend-optimized patch; bsdiff/xdelta3 results are labeled as an optimized
pairwise **proxy**. Full tables and framing:
[docs/ROUTE_BENCHMARKS.md](docs/ROUTE_BENCHMARKS.md),
[docs/BUTLER_COMPARISON.md](docs/BUTLER_COMPARISON.md),
[docs/OFFLINE_TOOLKIT.md](docs/OFFLINE_TOOLKIT.md).

### Delivery planner & optimized sidecars (v0.8.0)

```sh
# Publish a release in one pass: container + signature + plan +
# optimized sidecar, preceded by a preview (renames, blob warnings)
./target/release/cavs publish-dir ./Build_v2 --previous ./Build_v1 --out-dir releases/

# Per-file optimized sidecar for a hot pair, with the reasoning written out
./target/release/cavs optimize-patch --old ./Build_v1 --new ./Build_v2 \
  --algo auto --compression auto --explain-strategies why.md -o v1_to_v2.cavspatch

# Which pairs deserve a sidecar (never all O(N²) pairs)
./target/release/cavs patch-policy --versions v1,v2,...,v10 --distribution shares.json

# Pick the route for one client state under a device profile
./target/release/cavs route-plan --installed ./InstalledGame --new ./Build_v2 \
  --patch v1_to_v2.cavspatch --profile low-memory

# The proof report: every CAVS route vs the complete butler pipeline
# (default diff AND rediff --rediff-quality 9), honest verdicts included
./target/release/cavs bench full-pipeline --old ./Build_v1 --new ./Build_v2 \
  --butler-bin ./butler --include-pairwise --out results/pipeline

# Prove interrupted applies never break an install
./target/release/cavs test apply-recovery --old ./Build_v1 --new ./Build_v2
```

Measured on the 126 MiB directory release: CAVS auto-route ties the
optimized external patch on bytes (2.51 MiB) and apply time while using
**4.2× less memory** and generating **21× faster**; on a shifted 128 MiB
artifact it wins every column (4.21 KiB vs 11.39 KiB, 2.2× faster apply,
12% of the RAM); the compressed-blob weak case now routes through a
byte-level delta automatically (2.53 MiB where block routes paid
21.9 MiB). Planner and sidecar details:
[docs/DELIVERY_PLANNER.md](docs/DELIVERY_PLANNER.md),
[docs/PAIRWISE_SIDECARS.md](docs/PAIRWISE_SIDECARS.md).

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
  `sweep` cost report; inspect, verify, reconstruct, manage a global
  store (`add` / `rm` / `gc` / `stat` / `verify` / `export`, loose or
  packfile layout), inspect manifest formats
  (`manifest export` / `manifest bench`), diagnose deployments
  (`doctor`), run the corruption matrix (`test corrupt`) and generate/run
  reproducible large-build benchmarks (`bench gen` / `bench suite`). v0.7.0
  adds the offline toolkit (`signature` / `preview` / `diff-plan` / `apply` /
  `verify-install` / `file` / `ls`, `pack-dir` with `.cavsignore`,
  `optimize-patch`) and the external benchmark harnesses
  (`bench butler-offline` / `pairwise-proxy` / `routes` / `version-stream`).
  v0.8.0 adds the delivery planner (`route-plan`), per-file optimized
  sidecars (`optimize-patch --algo auto`, `apply-patch --memory-budget`),
  the hot-pair policy (`patch-policy`), one-command publishing
  (`publish-dir`), the full external pipeline harnesses
  (`bench butler-full` / `full-pipeline`) and the interrupted-apply matrix
  (`test apply-recovery`).
- **`cavs-server`**: stateful HTTP/HTTPS origin. Per-session have-set,
  inline/reference planning, dual-route decision (bootstrap vs chunks) per
  client, manifest format negotiation (compact binary v2 / JSON v1), CVSP
  binary batches, coalesced packfile range reads with read-efficiency
  metrics, immutable CDN-cacheable chunk and bootstrap endpoints (ETag,
  immutable Cache-Control, HTTP Range for resume), and a `--store` mode.
- **`cavs-client`**: native streaming client with a persistent cache and
  atomic, verified reconstruction; negotiates the compact binary manifest,
  takes the bootstrap route when offered (seeding its cache); resumes
  interrupted downloads from a crash-safe journal, retries transient
  failures with backoff, and maintains its own cache
  (`cache verify` / `repair` / `gc`).
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
