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

The benchmark harnesses (`cavs-bench`, the real-games scripts) and the raw
result data live in the full development repository, not in this open-source
tree. The measured summaries above are what those harnesses produced.
