# Apply chain report

Longer patch chains mean more sequential applies: more CPU, more intermediate state, and a larger failure surface (every intermediate patch must exist and apply cleanly).

| Policy | Avg steps | P95 steps | Max steps | Avg apply time |
|---|---:|---:|---:|---:|
| adjacent pairwise diffs | 1.54 | 4 | 9 | 38 ms |
| sparse dyadic ladder (aligned) | 1.20 | 3 | 4 | 29 ms |
| base hub (v06, bidirectional) | 1.76 | 2 | 2 | 52 ms |
| hot pairs (latest:3) + adjacent baseline | 1.42 | 4 | 7 | 35 ms |
| all-pairs theoretical one-hop baseline | 0.99 | 1 | 1 | 24 ms |
| CAVS content-addressed route | 1.00 | 1 | 1 | 64 ms |

CAVS routes and all-pairs patches are single-step by construction; adjacent chains grow with the version distance; the ladder bounds chains at O(log distance); base hubs need at most two steps but pay base drift.
