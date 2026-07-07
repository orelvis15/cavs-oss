# SteamPipe-style Build Analysis

> SteamPipe-style estimate based on public documentation. This is not Valve's exact SteamPipe implementation.

`dataset/Build_v1` → `dataset/Build_v2` (engine hint: auto)

| Metric | Value |
|---|---:|
| Old build | 125.83 MiB |
| New build | 126.89 MiB |
| SteamPipe-style estimate | 16.90 MiB |
| CAVS content-defined estimate | 4.42 MiB |
| Fixed 1 MiB reuse | 73.1% |
| Content-defined reuse | 96.5% |
| Files | 3 modified, 3 new, 2 deleted, 38 unchanged |

## Files ranked by update cost

| File | Status | Size | Fixed reuse | CDC reuse | SteamPipe-style | CAVS | Scatteredness |
|---|---|---:|---:|---:|---:|---:|---:|
| game.pck | modified | 76.81 MiB | 61.0% | 92.7% | 14.89 MiB | 2.89 MiB | 0.62 |
| assets/asset_21.dat | modified | 1.06 MiB | 0.0% | 0.0% | 512.55 KiB | 515.82 KiB | 0.00 |
| assets/asset_13_renamed.dat | new | 1.06 MiB | 0.0% | 100.0% | 512.55 KiB | 0 B | 0.00 |
| assets/asset_41.dat | new | 1.06 MiB | 0.0% | 0.0% | 512.55 KiB | 515.60 KiB | 0.00 |
| assets/asset_40.dat | new | 1.06 MiB | 0.0% | 0.0% | 512.54 KiB | 515.42 KiB | 0.00 |
| data/catalog.json | modified | 76.85 KiB | 0.0% | 0.0% | 11.38 KiB | 11.43 KiB | 0.00 |

## Findings

### Critical: Assets shifted or reordered inside the file

File:
  `game.pck`

Estimated wasted bytes: **12.00 MiB**

Why it happens:
  game.pck keeps 93% of its content (content-defined chunks) but only 61% of its fixed 1 MiB chunks — the bytes are there, at different offsets. Typical causes: reordered assets, a grown asset shifting everything after it, or non-deterministic packing.

Recommended fix:
  Keep a stable asset order, pad or align entries so unrelated assets keep their offsets, and avoid full repacks for small changes.

Expected improvement:
  Up to 12.00 MiB of the estimated download is misalignment, not new content, and would disappear with stable offsets.

### Warning: Scattered changes across a pack file

File:
  `game.pck`

Estimated wasted bytes: **12.00 MiB**

Why it happens:
  game.pck changed in 30 of 77 1 MiB windows across 19 runs (scatteredness 0.62). Fixed 1 MiB chunking cannot reuse windows whose content moved or interleaves edits.

Recommended fix:
  Group assets by level/feature, keep asset ordering stable between builds and add new content as new packs.

Expected improvement:
  Changes collapse into few contiguous regions, so fixed-chunk updates only ship those regions.
