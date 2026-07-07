# Route Certification

Result: **PASS**

Policy: `balanced` — weights: apply_ms=0.05, build_ms=0, disk_read=0.01, network=10, ram_mb=0.05, temp_disk=0.01

## Decisions per client state

| Client state | Recommended route | Network | Apply | RAM | Reason |
|---|---|---:|---:|---:|---|
| cold-install | bootstrap | 62.14 MiB | 253 ms | 32.00 MiB | 62.14 MiB (estimated) over the wire · ~32.00 MiB peak RAM · 126.89 MiB temp disk · policy balanced |
| cold-cache-previous | .cavsplan | 2.26 MiB | 253 ms | 40.00 MiB | 2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced |
| warm-cache | .cavsplan | 2.26 MiB | 253 ms | 40.00 MiB | 2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced |
| exact-previous-version | .cavsplan | 2.26 MiB | 253 ms | 40.00 MiB | 2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced |
| low-ram | .cavsplan | 2.26 MiB | 253 ms | 40.00 MiB | 2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced |
| slow-hdd | .cavsplan | 2.26 MiB | 253 ms | 40.00 MiB | 2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced |
| limited-disk | .cavsplan | 2.26 MiB | 253 ms | 40.00 MiB | 2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced |

## Measured route matrix

# Delivery route comparison

`dataset/Build_v1` → `dataset/Build_v2` (directory mode, 125.83 MiB → 126.89 MiB)

| Route | Network bytes | Diff time | Apply time | Peak RSS | Output OK | Notes |
|---|---:|---:|---:|---:|---|---|
| full download (raw) | 126.89 MiB | — | 0 ms | — | yes | no old-version reuse |
| full zstd-19 (CAVS bootstrap) | 62.12 MiB | 4074 ms | 10 ms | — | yes | cache-less first install |
| CAVS chunk / hybrid (wire) | 4.41 MiB | 258 ms | — | — | yes | 80 of 1093 chunks new; same bytes for warm cache or cold cache + previous install (hybrid) |
| CAVS offline plan (.cavsplan) | 2.26 MiB | 506 ms | 214 ms | — | yes | portable patch: signature diff + zstd-19 payload, journaled apply |
| pairwise proxy: bsdiff+zstd-19 | 2.77 MiB | 25065 ms | 3516 ms | 1468 MiB | yes | one exact old→new pair only (proxy) |
| pairwise proxy: bsdiff+brotli-9 | 2.77 MiB | 25065 ms | 3516 ms | 1468 MiB | yes | one exact old→new pair only (proxy) |
| pairwise proxy: xdelta3+zstd-19 | 2.76 MiB | 4013 ms | 3510 ms | 396 MiB | yes | one exact old→new pair only (proxy) |
| pairwise proxy: xdelta3+brotli-9 | 2.76 MiB | 4013 ms | 3510 ms | 396 MiB | yes | one exact old→new pair only (proxy) |

> skipped: butler offline: no --butler-bin given
> skipped: pairwise patches serve exactly one old→new pair; storage and generation cost grow with every published pair

## Checks

| Check | Result | Details |
|---|---|---|
| state: cold-install | PASS | bootstrap — 62.14 MiB network, 253 ms apply (62.14 MiB (estimated) over the wire · ~32.00 MiB peak RAM · 126.89 MiB temp disk · policy balanced) |
| state: cold-cache-previous | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: warm-cache | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: exact-previous-version | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: low-ram | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: slow-hdd | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: limited-disk | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| measured routes verified | PASS | 8 routes measured, 2 skipped (missing tools) |
| skipped: butler offline: no --butler-bin given | SKIPPED | optional dependency not installed — skipped, never selected |
| skipped: pairwise patches serve exactly one old→new pair; storage and generation cost grow with every published pair | SKIPPED | optional dependency not installed — skipped, never selected |

Recommended route: **CAVS offline plan (.cavsplan)**

Why: 2.26 MiB network — the smallest verified payload; 214 ms apply; streaming memory (no full old copy in RAM); byte-identical output

Rules: a route is never chosen when its dependency is unavailable, when it fails verification, or when it exceeds the policy limits; near-ties prefer the simpler, lower-risk route.
