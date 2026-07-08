# Patch policy benchmark

10 versions (v01 … v10), pairwise engine **bsdiff**, traffic model **adjacent-heavy** (100000 users), client state **warm-cache**.

Pairwise diffs are not a single strategy. This benchmark compares several practical patch graph policies: adjacent-only, sparse power-of-two ladder, base-version, hot-pair, and all-pairs. The all-pairs graph is included only as a theoretical one-hop baseline.

| Policy | Patch count | Storage | Avg update | P95 update | P99 update | Max steps | Build time | Coverage | Notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| adjacent pairwise diffs | 9 | 4.23 MiB | 897.55 KiB | 2.02 MiB | 4.23 MiB | 9 | 55.8s | 100.0% | O(N) storage; skips chain patches |
| sparse dyadic ladder (aligned) | 16 | 14.71 MiB | 882.75 KiB | 1.89 MiB | 3.79 MiB | 5 | 118.2s | 100.0% | <2N storage (aligned); O(log distance) chains |
| base hub (v06, bidirectional) | 18 | 21.59 MiB | 2.13 MiB | 3.79 MiB | 3.91 MiB | 2 | 167.0s | 100.0% | auto-selected over v01, v10 under the adjacent-heavy traffic model |
| hot pairs (latest:3) + adjacent baseline | 11 | 6.44 MiB | 893.96 KiB | 2.02 MiB | 4.17 MiB | 7 | 71.9s | 100.0% | budget 2x-latest-build (32.03 MiB); 2 of 2 hot edges kept |
| all-pairs theoretical one-hop baseline | 45 | 71.07 MiB | 872.37 KiB | 1.83 MiB | 3.60 MiB | 2 | 472.7s | 100.0% | O(N²) storage; not a normal production policy |
| CAVS content-addressed route | content store | 29.84 MiB | 2.26 MiB | 5.12 MiB | 8.95 MiB | 1 | 0.4s | 100.0% | no patch graph; chunk store serves any jump |

Storage is the sum of stored patch bytes for the policy (deduplicated chunk store for CAVS). Avg/P95/P99 update bytes are weighted by the traffic model; uncovered queries fall back to a full compressed download and count against coverage.
