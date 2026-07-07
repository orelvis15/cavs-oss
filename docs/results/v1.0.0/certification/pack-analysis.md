# Pack Analysis

> SteamPipe-style estimate based on public documentation. This is not Valve's exact SteamPipe implementation.

`dataset/Build_v1` → `dataset/Build_v2`

| File | Size | Changed windows (1 MiB) | Scatteredness | Entropy | Fixed reuse | CDC reuse | Main issue | Recommendation |
|---|---:|---:|---:|---:|---:|---:|---|---|
| game.pck | 76.81 MiB | 30 | 0.62 | 6.90 | 61.0% | 92.7% | asset_shuffling | keep asset order stable |
| assets/asset_21.dat | 1.06 MiB | 2 | 0.00 | 7.83 | 0.0% | 0.0% | localized | OK |
| assets/asset_13_renamed.dat | 1.06 MiB | 2 | 0.00 | 7.83 | 0.0% | 100.0% | localized | OK |
| assets/asset_41.dat | 1.06 MiB | 2 | 0.00 | 7.81 | 0.0% | 0.0% | localized | OK |
| assets/asset_40.dat | 1.06 MiB | 2 | 0.00 | 7.81 | 0.0% | 0.0% | localized | OK |

## Findings

- **[critical] Assets shifted or reordered inside the file** (`game.pck`) — Keep a stable asset order, pad or align entries so unrelated assets keep their offsets, and avoid full repacks for small changes.

- **[warning] Scattered changes across a pack file** (`game.pck`) — Group assets by level/feature, keep asset ordering stable between builds and add new content as new packs.
