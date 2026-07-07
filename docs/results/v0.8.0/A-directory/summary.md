# Full-pipeline comparison

`/private/tmp/claude-501/-Users-l41777-Documents-repositories-bitlakelab-code/e62c89dc-b5dd-4c8d-aad3-ba9fbd887de4/scratchpad/ds-dir/Build_v1` → `/private/tmp/claude-501/-Users-l41777-Documents-repositories-bitlakelab-code/e62c89dc-b5dd-4c8d-aad3-ba9fbd887de4/scratchpad/ds-dir/Build_v2` (directory mode, 125.83 MiB → 126.89 MiB)

| Route | Download | Generate | Apply | Gen RSS | Apply RSS | Output | Notes |
|---|---:|---:|---:|---:|---:|---|---|
| full download (raw) | 126.89 MiB | — | 0 ms | — | — | OK | no reuse |
| full zstd-19 (bootstrap) | 62.12 MiB | 4514 ms | — | — | — | OK | cache-less first install |
| CAVS chunks / hybrid (wire) | 5.42 MiB | 332 ms | — | — | — | OK | warm cache, or cold cache + previous install |
| CAVS offline plan (.cavsplan) | 2.51 MiB | 539 ms | 391 ms | — | 23 MiB | OK | streaming journaled apply |
| CAVS optimized sidecar (.cavspatch) | 2.51 MiB | 29633 ms | 991 ms | — | 22 MiB | OK | per-file: 39 copy-old / 1 plan / 1 bsdiff / 0 xdelta3 / 3 full |
| CAVS auto-route | 2.51 MiB | 539 ms | 391 ms | — | 23 MiB | OK | planner picks: CAVS offline plan (.cavsplan) |
| butler diff (default) | 2.52 MiB | 1006 ms | 331 ms | 33 MiB | 35 MiB | OK | +61.72 KiB signature |
| butler rediff q9 (optimized) | 2.51 MiB | 10549 ms | 403 ms | 942 MiB | 97 MiB | OK | bsdiff + high-quality recompression |
| pairwise proxy: bsdiff+zstd-19 | 3.02 MiB | 25040 ms | 3491 ms | 1357 MiB | — | OK | one exact pair only |
| pairwise proxy: bsdiff+brotli-9 | 3.02 MiB | 25040 ms | 3491 ms | 1357 MiB | — | OK | one exact pair only |
| pairwise proxy: xdelta3+zstd-19 | 3.01 MiB | 4063 ms | 3425 ms | 397 MiB | — | OK | one exact pair only |
| pairwise proxy: xdelta3+brotli-9 | 3.01 MiB | 4063 ms | 3425 ms | 397 MiB | — | OK | one exact pair only |

## Verdict (CAVS auto-route vs the optimized patch pipeline)

- network bytes: tie (within 5%)
- apply time: tie (within 5%)
- apply peak RAM: CAVS wins (24% of the optimized pipeline)
- generate time: CAVS wins (5% of the optimized pipeline)
- storage model: CAVS serves any version jump from one immutable store; pairwise patches serve exactly one pair each

> skipped: pairwise patches serve exactly one old→new pair; storage and generation cost grow with every published pair
