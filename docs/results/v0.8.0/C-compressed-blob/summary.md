# Full-pipeline comparison

`/private/tmp/claude-501/-Users-l41777-Documents-repositories-bitlakelab-code/e62c89dc-b5dd-4c8d-aad3-ba9fbd887de4/scratchpad/ds-blob/game_v1.tar.zst` → `/private/tmp/claude-501/-Users-l41777-Documents-repositories-bitlakelab-code/e62c89dc-b5dd-4c8d-aad3-ba9fbd887de4/scratchpad/ds-blob/game_v2.tar.zst` (artifact mode, 61.67 MiB → 62.17 MiB)

| Route | Download | Generate | Apply | Gen RSS | Apply RSS | Output | Notes |
|---|---:|---:|---:|---:|---:|---|---|
| full download (raw) | 62.17 MiB | — | 0 ms | — | — | OK | no reuse |
| full zstd-19 (bootstrap) | 62.17 MiB | 7572 ms | — | — | — | OK | cache-less first install |
| CAVS chunks / hybrid (wire) | 23.74 MiB | 135 ms | — | — | — | OK | warm cache, or cold cache + previous install |
| CAVS offline plan (.cavsplan) | 21.92 MiB | 2747 ms | 577 ms | — | 52 MiB | OK | streaming journaled apply |
| CAVS optimized sidecar (.cavspatch) | 2.53 MiB | 32463 ms | 319 ms | — | 74 MiB | OK | per-file: 0 copy-old / 0 plan / 0 bsdiff / 1 xdelta3 / 0 full |
| CAVS auto-route | 2.53 MiB | 32463 ms | 319 ms | — | 74 MiB | OK | planner picks: CAVS optimized sidecar (.cavspatch) |
| butler diff (default) | 21.92 MiB | 801 ms | 247 ms | 56 MiB | 54 MiB | OK | +30.12 KiB signature |
| butler rediff q9 (optimized) | 2.90 MiB | 8851 ms | 294 ms | 813 MiB | 92 MiB | OK | bsdiff + high-quality recompression |
| pairwise proxy: bsdiff+zstd-19 | 2.54 MiB | 16461 ms | 306 ms | 1161 MiB | — | OK | one exact pair only |
| pairwise proxy: bsdiff+brotli-9 | 2.54 MiB | 16461 ms | 306 ms | 1161 MiB | — | OK | one exact pair only |
| pairwise proxy: xdelta3+zstd-19 | 2.53 MiB | 669 ms | 177 ms | 396 MiB | — | OK | one exact pair only |
| pairwise proxy: xdelta3+brotli-9 | 2.53 MiB | 669 ms | 177 ms | 396 MiB | — | OK | one exact pair only |

## Verdict (CAVS auto-route vs the optimized patch pipeline)

- network bytes: CAVS wins (87% of the optimized pipeline)
- apply time: optimized pipeline wins (CAVS at 109%)
- apply peak RAM: CAVS wins (80% of the optimized pipeline)
- generate time: optimized pipeline wins (CAVS at 367%)
- storage model: CAVS serves any version jump from one immutable store; pairwise patches serve exactly one pair each

> skipped: pairwise patches serve exactly one old→new pair; storage and generation cost grow with every published pair
