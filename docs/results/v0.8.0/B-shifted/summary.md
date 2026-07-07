# Full-pipeline comparison

`/private/tmp/claude-501/-Users-l41777-Documents-repositories-bitlakelab-code/e62c89dc-b5dd-4c8d-aad3-ba9fbd887de4/scratchpad/ds-art/v1.bin` → `/private/tmp/claude-501/-Users-l41777-Documents-repositories-bitlakelab-code/e62c89dc-b5dd-4c8d-aad3-ba9fbd887de4/scratchpad/ds-art/v2-shifted.bin` (artifact mode, 128.00 MiB → 128.00 MiB)

| Route | Download | Generate | Apply | Gen RSS | Apply RSS | Output | Notes |
|---|---:|---:|---:|---:|---:|---|---|
| full download (raw) | 128.00 MiB | — | 0 ms | — | — | OK | no reuse |
| full zstd-19 (bootstrap) | 64.06 MiB | 5719 ms | — | — | — | OK | cache-less first install |
| CAVS chunks / hybrid (wire) | 10.90 KiB | 283 ms | — | — | — | OK | warm cache, or cold cache + previous install |
| CAVS offline plan (.cavsplan) | 4.21 KiB | 324 ms | 170 ms | — | 11 MiB | OK | streaming journaled apply |
| CAVS optimized sidecar (.cavspatch) | 4.23 KiB | 44109 ms | 551 ms | — | 12 MiB | OK | per-file: 0 copy-old / 1 plan / 0 bsdiff / 0 xdelta3 / 0 full |
| CAVS auto-route | 4.21 KiB | 324 ms | 170 ms | — | 11 MiB | OK | planner picks: CAVS offline plan (.cavsplan) |
| butler diff (default) | 68.13 KiB | 807 ms | 163 ms | 26 MiB | 21 MiB | OK | +61.96 KiB signature |
| butler rediff q9 (optimized) | 11.39 KiB | 11259 ms | 370 ms | 1894 MiB | 94 MiB | OK | bsdiff + high-quality recompression |
| pairwise proxy: bsdiff+zstd-19 | 4.59 KiB | 24783 ms | 222 ms | 2441 MiB | — | OK | one exact pair only |
| pairwise proxy: bsdiff+brotli-9 | 4.61 KiB | 24783 ms | 222 ms | 2441 MiB | — | OK | one exact pair only |
| pairwise proxy: xdelta3+zstd-19 | 4.34 KiB | 470 ms | 364 ms | 395 MiB | — | OK | one exact pair only |
| pairwise proxy: xdelta3+brotli-9 | 4.42 KiB | 470 ms | 364 ms | 395 MiB | — | OK | one exact pair only |

## Verdict (CAVS auto-route vs the optimized patch pipeline)

- network bytes: CAVS wins (37% of the optimized pipeline)
- apply time: CAVS wins (46% of the optimized pipeline)
- apply peak RAM: CAVS wins (12% of the optimized pipeline)
- generate time: CAVS wins (3% of the optimized pipeline)
- storage model: CAVS serves any version jump from one immutable store; pairwise patches serve exactly one pair each

> skipped: pairwise patches serve exactly one old→new pair; storage and generation cost grow with every published pair
