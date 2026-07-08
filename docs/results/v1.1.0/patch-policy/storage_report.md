# Storage report

Latest build: 32.00 MiB raw, 16.02 MiB compressed.

| Policy | Patch count | Storage | Storage / latest build | Total served |
|---|---:|---:|---:|---:|
| adjacent pairwise diffs | 9 | 4.20 MiB | 0.26× | 85.01 GiB |
| sparse dyadic ladder (aligned) | 16 | 14.59 MiB | 0.91× | 83.61 GiB |
| base hub (v06, bidirectional) | 18 | 21.41 MiB | 1.34× | 206.55 GiB |
| hot pairs (latest:3) + adjacent baseline | 11 | 6.39 MiB | 0.40× | 84.67 GiB |
| all-pairs theoretical one-hop baseline | 45 | 70.49 MiB | 4.40× | 82.64 GiB |
| CAVS content-addressed route | content store | 29.84 MiB | 1.86× | 220.66 GiB |

## Hot-pair storage budget

Budget: 32.03 MiB. Greedy selection by expected bytes saved per stored byte; a patch is kept only when it beats its fallback route.

| Pair | Patch | Fallback route | Traffic share | Kept |
|---|---:|---:|---:|---|
| v07→v10 | 1.25 MiB | 1.32 MiB | 0.71% | yes |
| v08→v10 | 961.51 KiB | 961.90 KiB | 0.71% | yes |
