# Patch policy benchmark

10 versions (v01 … v10), pairwise engine **cavsplan**, traffic model **adjacent-heavy** (100000 users), client state **cold-cache-with-previous-install**.

Pairwise diffs are not a single strategy. This benchmark compares several practical patch graph policies: adjacent-only, sparse power-of-two ladder, base-version, hot-pair, and all-pairs. The all-pairs graph is included only as a theoretical one-hop baseline.

| Policy | Patch count | Storage | Avg update | P95 update | P99 update | Max steps | Build time | Coverage | Notes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| adjacent pairwise diffs | 9 | 4.20 MiB | 891.36 KiB | 2.00 MiB | 4.20 MiB | 9 | 0.9s | 100.0% | O(N) storage; skips chain patches |
| sparse dyadic ladder (aligned) | 16 | 14.59 MiB | 876.73 KiB | 1.88 MiB | 3.76 MiB | 4 | 2.0s | 100.0% | <2N storage (aligned); O(log distance) chains |
| base hub (v06, bidirectional) | 18 | 21.41 MiB | 2.12 MiB | 3.76 MiB | 3.88 MiB | 2 | 2.6s | 100.0% | auto-selected over v01, v10 under the adjacent-heavy traffic model |
| hot pairs (latest:3) + adjacent baseline | 11 | 6.39 MiB | 887.85 KiB | 2.00 MiB | 4.13 MiB | 7 | 1.1s | 100.0% | budget 2x-latest-build (32.03 MiB); 2 of 2 hot edges kept |
| all-pairs theoretical one-hop baseline | 45 | 70.49 MiB | 866.50 KiB | 1.82 MiB | 3.57 MiB | 1 | 8.2s | 100.0% | O(N²) storage; not a normal production policy |
| CAVS content-addressed route | content store | 29.84 MiB | 2.26 MiB | 5.12 MiB | 8.95 MiB | 1 | 0.4s | 100.0% | no patch graph; chunk store serves any jump |

Storage is the sum of stored patch bytes for the policy (deduplicated chunk store for CAVS). Avg/P95/P99 update bytes are weighted by the traffic model; uncovered queries fall back to a full compressed download and count against coverage.
