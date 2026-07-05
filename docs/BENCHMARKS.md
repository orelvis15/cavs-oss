# Benchmarks

All numbers are **measured**, not projected. Update payloads were captured over
real HTTP sessions (`cavs-server` + `cavs-client`). Game numbers come from real
open-source Godot projects exported to PCK at two real points in their git
history. "Update" means what a player who already has the previous version
downloads; the baseline is downloading the full new release compressed with
zstd -3.

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

## Honest negatives (video suite)

CAVS is not a codec and doesn't pretend to be:

- **Single video, first play**: ~0% savings, packaging overhead +0.03%.
- **ABR ladder** (same title, multiple bitrates): ~0% cross-bitrate dedup —
  different bitstreams share no bytes. Expected and reported.
- **Already-compressed files** (JPEG, MP4, ZIP): overhead +0.08%, near-zero
  savings — the value is elsewhere.
- Where redundancy *does* cross content (shared episode intros): 7–21% storage
  savings by codec; warm re-watch: −100% egress.

## Reproducing

The benchmark harnesses (`cavs-bench`, the real-games scripts) and the raw
result data live in the full development repository, not in this open-source
tree. The measured summaries above are what those harnesses produced.
