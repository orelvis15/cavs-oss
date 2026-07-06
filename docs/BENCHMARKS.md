# Benchmarks

All numbers are **measured**, not projected. Update payloads were captured over
real HTTP sessions (`cavs-server` + `cavs-client`). Game numbers come from real
open-source Godot projects exported to PCK at two real points in their git
history. "Update" means what a player who already has the previous version
downloads; the baseline is downloading the full new release compressed with
zstd -3.

## Real games — cold install (dual delivery route)

First install used to be CAVS's weak spot: chunk-level compression cost +2–4%
over downloading the whole release as one `.zst`. The **dual route** removes
it: `cavs pack --bootstrap` also emits the full artifact zstd-19-compressed,
the server offers it to cold clients whenever it beats the chunk path, and the
client **seeds its chunk cache from it** — so the next update is incremental
with zero extra downloads.

| Game | PCK v1 | Full zstd-3 | Chunk path (old cold) | **Dual route cold** | vs zstd-3 |
|---|---:|---:|---:|---:|---:|
| godotengine/tps-demo | 569.15 MiB | 247.62 MiB | 251.91 MiB (+1.7%) | **221.42 MiB** | **−10.6%** |
| GDQuest 3D third-person | 61.09 MiB | 27.66 MiB | 28.20 MiB (+2.0%) | **24.43 MiB** | **−11.7%** |
| MechanicalFlower/Marble | 9.59 MiB | 6.55 MiB | 6.68 MiB (+2.0%) | **5.68 MiB** | **−13.2%** |
| Godot 4.7 export suite | 5.39 MiB | 4.52 MiB | 4.71 MiB (+4.1%) | **4.20 MiB** | **−7.1%** |

The server routed all four games to the bootstrap automatically (its per-
session estimate vs the 2% threshold), an incompressible payload correctly
stays on the chunk path, and every reconstruction was byte-identical.

## Real games — update payload

| Game | Update | Full (zstd) | CAVS (64 KiB) | Saved |
|---|---|---:|---:|---:|
| godotengine/tps-demo (569 MB) | tag 4.5 → master (7 files) | 247.60 MiB | **1.64 MiB** | **−99.3%** |
| MechanicalFlower/Marble | 1.6.0 → 1.6.1 | 6.55 MiB | 0.14 MiB | **−97.9%** |
| GDQuest 3D third-person | HEAD~10 → HEAD (468 files) | 27.61 MiB | 8.70 MiB | **−68.5%** |

- Re-downloading the same version is **0 bytes** of payload (everything
  resolves from the persistent cache as references).
- All reconstructions were **byte-identical** (SHA-256 verified), and the Godot
  runtime mounted the reconstructed PCKs via `load_resource_pack()`.
- A client that cold-installed via the bootstrap route pays the same update
  prices: cache seeding reproduces the exact chunk plan of the served version.

## Compact manifest (v0.3.0) — metadata overhead

The runtime manifest used to travel as JSON. The binary v2 format (`CAVSMF2`,
served by content negotiation, JSON kept for compatibility) stores each unique
chunk hash once and references it by varint index. Measured on the same real
games (64 KiB CDC, `cavs manifest bench` + real HTTP sessions):

| Game | Manifest JSON v1 | Binary v2 | Saved | Parse v1 → v2 |
|---|---:|---:|---:|---|
| godotengine/tps-demo | 894.2 KiB | **208.5 KiB** | **−76.7%** | 0.53 → 0.49 ms |
| GDQuest 3D third-person | 103.1 KiB | **24.8 KiB** | **−75.9%** | 0.062 → 0.058 ms |
| MechanicalFlower/Marble | 20.3 KiB | **5.1 KiB** | **−75.0%** | 0.018 → 0.017 ms |

Chunk-path bytes are unchanged (the tps-demo update is still 1.64 MiB), so the
improvement lands where metadata dominates: a warm re-fetch — the "is there an
update?" check every launcher does — now costs ~75% less wire, and total
update egress improves up to −26.6% (tps-demo: manifest was a third of the
update cost). Cold installs, warm re-fetch = 0 payload bytes and byte-identical
reconstruction were re-verified on all three games.

## CAVS vs dedicated delta tools

Update payload v1→v2, in MiB. Per-pair deltas win on raw bytes — that is not
the point; the point is operational cost.

| Game | zstd full | zip full | rsync wire | rdiff | xdelta3 -9 | bsdiff | **CAVS (64k)** |
|---|---:|---:|---:|---:|---:|---:|---:|
| tps-demo | 247.60 | 247.51 | 462.95 | 0.70 | 0.03 | 0.03 | **1.64** |
| GDQuest | 27.61 | 27.42 | 54.86 | 7.06 | 3.78 | 3.82 | **8.70** |
| Marble | 6.55 | 6.34 | 0.02 | 0.01 | 0.00 | 0.00 | **0.14** |

Reading the table:

- **rsync loses on large binaries with scattered changes**: for tps-demo it
  transmitted 462.95 MiB — *worse than downloading the whole thing*. Block
  checksums aren't built for this.
- **xdelta3/bsdiff produce the smallest patches** for a single version pair —
  but you need one patch per pair (v1→v5, v2→v5, v3→v5…, O(N²)), and generating
  the bsdiff patch for tps-demo cost **137 s and 9.1 GB of RAM**. CAVS packs
  **once per release (3.5 s)** and the same chunk store serves any version
  jump, with resumable, CDN-cacheable, cross-version reuse.

## Parameter sweeps

Chunk size is the strongest lever. Dropping the FastCDC average from 256 KiB to
64 KiB cut the tps-demo update from 4.97 → **1.64 MiB** (3×) for +1.3% storage.
zstd level 3 was the sweet spot (level 5 saved ~1.5% more cold egress for +33%
packing time). These results set the current defaults: **FastCDC 64 KiB +
zstd 3**.

Since 0.1.2 the sweep is built in: `cavs sweep new.pck --prev old.cavs`
measures six candidate profiles (fixed 256K/512K/1M, FastCDC 64K/128K/256K) on
the real bytes — chunk counts, sampled compression, manifest weight and real
chunk reuse — and `cavs pack --profile auto` applies the cheapest. It catches
per-title cases a fixed default cannot: for Marble (an in-place patch with no
byte shifting) `fixed-256k` beats `fastcdc-64k` on update egress, 72 vs
153 KiB. Passing `--prev` the *published* `.cavs` keeps the choice consistent
across a version stream, which is what preserves chunk reuse.

## Client cost (tps-demo update, 569 MB, release binaries)

| Metric | Before | After (streaming) |
|---|---:|---:|
| Peak client RAM | 1124 MiB | **7 MiB** |
| Update CPU | 7.0 s | **2.0 s** |
| Pack time (whole release) | 40 s | **3.5 s** |

The client reconstructs by streaming to disk (batch decoded from the socket →
disk cache → `.part` → SHA-256 → atomic rename), so RAM is constant regardless
of game size.

## Global store — storage dedup at rest

Ingesting two versions of a real game (Marble 1.6.0 and 1.6.1) into the global
content-addressable store stored the shared chunks once: **13.80 MiB logical →
7.04 MiB on disk = ~49% less storage** than keeping each `.cavs` separately,
while serving each version byte-identically over HTTP. Garbage collection
reclaims chunks that no published version references.

## Packfile storage (v0.4.0) — operational shape at rest

`store add --storage packfiles` keeps the same chunks in a few immutable
content-addressed `.cavspack` files instead of one file per chunk, and the
server coalesces each batch's pack reads (nearby chunks = one physical read).
Same two-version stores as above, loose vs packfiles, full
cold + update + warm session per layout:

| Game | Chunk objects on disk | Physical reads (whole session) | Read amplification |
|---|---|---|---|
| MechanicalFlower/Marble | 130 → **4** | 130 → **2** (65×) | 1.000 |
| GDQuest 3D third-person | 807 → **4** | 805 → **7** (115×) | 1.000 |
| godotengine/tps-demo | 5,775 → **6** | 5,775 → **34** (170×) | 1.000 |

Amplification 1.000 means coalescing read **zero** extra bytes: chunks are
written in reconstruction order, so merged ranges are exactly contiguous.
Wire bytes, routing and byte-identical reconstruction are unchanged vs the
loose layout (and vs 0.3.0) — the win is operational: fewer objects to
store/upload/list, fewer syscalls to serve, CDN-ready immutable packs
(`store export` emits the deterministic object tree). Store disk size also
drops slightly (tps-demo: −3.9%, filesystem block overhead of thousands of
small files).

## Hardening (v0.5.0) — recovery, measured

v0.5.0 changes no wire format and no routing: the cold/update/warm numbers
above were re-measured after it and are byte-for-byte identical (tps-demo
update still 1.64 MiB inline, warm still 0 bytes, all reconstructions
byte-identical). What it adds is resilience, and that was measured too:

- **Interrupted install resume** (tps-demo, 232 MiB bootstrap artifact):
  the client was killed with `kill -9` at 57 MiB downloaded. The next
  fetch found the journal, hashed the partial file, continued with an HTTP
  `Range` request and downloaded only the remaining **166.5 MiB** — final
  file byte-identical, journal and partials cleaned up. A stale journal
  (asset republished) is discarded and the fetch starts clean.
- **Cache self-repair** (real cache, 5,747 chunks / 510 MiB): 3 entries
  were corrupted on disk. `cache verify` detected and quarantined exactly
  those 3 (`CAVS-E-CACHE-CORRUPT-RECOVERABLE`), `cache repair` re-fetched
  exactly those 3, and the following re-fetch was back to **0 payload
  bytes** and byte-identical.
- **Corruption matrix**: `cavs test corrupt` runs ~20 targeted mutations
  (container magic/sections/data/truncation, manifest header/body/
  truncation, overlong varints, bootstrap sidecar, packfile header/data/
  footer/index, out-of-range reads) against real game containers — every
  corrupted artifact is rejected cleanly on all three games. The same
  invariants are fuzzed (5 libFuzzer targets) and replayed
  deterministically in CI: full byte-flip sweeps over the pack index and
  container leave **zero** unauthenticated-content survivors.
- **Client memory** (release build, 569 MB game): peak RSS **14.3 MiB**
  for the cold bootstrap install and **6.3 MiB** for the update — still
  ~constant with asset size.

## Synthetic large builds (v0.5.0) — reproducible suite

`cavs bench gen` emits a deterministic dataset (same seed ⇒ identical bytes
on any machine): a base build plus the update shapes that matter for chunked
delivery. `cavs bench suite` packs and measures every version. 1 GiB base,
FastCDC 64 KiB + zstd 3, Apple Silicon laptop:

| Update shape | Changed | Pack | Update egress |
|---|---|---:|---:|
| v2-small | ~3% of blocks | 6.4 s | 51.3 MiB (5.0%) |
| v2-medium | ~15% | 6.8 s | 216.6 MiB (21.2%) |
| v2-large | ~50% | 6.8 s | 449.1 MiB (43.9%) |
| v2-shifted | 4 KiB inserted at head — **every byte shifts** | 7.3 s | **10.9 KiB (0.0%)** |
| v2-reordered | same blocks, 8 MiB groups swapped | 9.1 s | 20.8 MiB (2.0%) |

The `v2-shifted` row is the reason content-defined chunking exists: a
byte-offset shift that would invalidate every fixed block costs effectively
nothing. Update egress tracks the changed fraction linearly, and manifests
stay compact (1.28 MiB JSON → 311 KiB binary v2 for ~8,600 chunks).
Ingesting v1 + v2-small into a packfile store yields 6 immutable packs.

## Hybrid reconstruction (v0.6.0) — previous install as a byte source

128 MiB synthetic suite (`bench gen --size 128MiB --seed 5`), fastcdc-64k,
release binaries, localhost server. "Cold" = empty chunk cache; "cold +
previous" = empty cache but the old build on disk (the situation every
pairwise patcher assumes — and where v0.5 had to pay the full artifact).

| Update variant | v0.5 cold | v0.6 cold + previous install | Reduction | Warm cache (v0.5 = v0.6) |
|---|---:|---:|---:|---:|
| small change | 64.55 MiB | **6.24 MiB** | **−90.3 %** | 6.24 MiB |
| medium change | 64.56 MiB | **26.53 MiB** | **−58.9 %** | 26.53 MiB |
| shifted (every byte moves) | 64.56 MiB | **10.9 KiB** | **−99.98 %** | 10.9 KiB |

- Warm-cache wire bytes are identical to v0.5 — the unified plan executor
  introduces **no regression** on any existing path (152 workspace tests,
  including the full v0.5 e2e suites, pass unchanged).
- Every copied range is BLAKE3-verified before writing; final SHA-256 gate
  unchanged; all outputs byte-identical (`cmp`).
- Range coalescing: shifted variant plans 1,082 chunk ops → 18 contiguous
  reads (60×); small 1,087 → 154 (7×); strict contiguity, read
  amplification 1.0.
- A previous install with a corrupted byte demotes exactly the affected
  range to network (`CAVS-E-PREVIOUS-ARTIFACT-MISMATCH`) and still ends
  byte-identical.
- No-op re-fetch of an already-current install: **0 payload bytes**,
  ~0.4 s (manifest + one local SHA-256).
- `.cavssig` signature of the 128 MiB build: 88 KiB (0.067 %),
  deterministic, one flipped source byte detected.

## CAVS vs delta patching — v0.6.0 baseline

Block-based delta model (fixed 64 KiB blocks, weak rolling hash + BLAKE3
confirmation, COPY/DATA planning with coalescing, zstd-1 transport), plus
`xdelta3 -9` and `bsdiff` as byte-level baselines, same suite:

| Pair | Full re-download | Block-delta patch | xdelta3 -9 | bsdiff | CAVS update |
|---|---:|---:|---:|---:|---:|
| small | 64.05 MiB | 1.94 MiB | 1.94 MiB | 1.96 MiB | 6.06 MiB |
| medium | 64.05 MiB | 8.83 MiB | 8.82 MiB | 8.90 MiB | 26.28 MiB |
| shifted | 64.06 MiB | 4.04 KiB | 4.65 KiB | 4.67 KiB | 10.90 KiB |

Pairwise patches win per-pair bytes (inherent: they ship dirty regions,
CAVS ships whole chunks); CAVS packages once per release, serves every
version jump from one deduplicated, CDN-cacheable store, and resumes and
self-repairs. Full tables, timings and framing:
[DELTA_COMPARISON.md](DELTA_COMPARISON.md). Compression cross-check
(`bench compression`): zstd and Brotli within 0.1 % on size here, zstd
~40× faster decode — zstd-3 stays the default.

## Honest negatives (video suite)

CAVS is not a codec and doesn't pretend to be:

- **Single video, first play**: ~0% savings, packaging overhead +0.03%.
- **ABR ladder** (same title, multiple bitrates): ~0% cross-bitrate dedup —
  different bitstreams share no bytes. Expected and reported.
- **Already-compressed files** (JPEG, MP4, ZIP): overhead +0.08%, near-zero
  savings — the value is elsewhere. The payload classifier now detects these
  (entropy + zstd probe) and packs them with large fixed chunks so the
  overhead stays minimal instead of paying for useless small chunks.
- Where redundancy *does* cross content (shared episode intros): 7–21% storage
  savings by codec; warm re-watch: −100% egress.

## Reproducing

The synthetic large-build suite is fully reproducible from this tree:
`cavs bench gen --out ds --size 1GiB && cavs bench suite --dataset ds --out
results` regenerates the exact same dataset (deterministic PRNG) and writes
`summary.md`/`summary.json`. The real-game harnesses (`cavs-bench`, the
real-games scripts) and their raw result data live in the full development
repository, not in this open-source tree; the measured summaries above are
what those harnesses produced.
