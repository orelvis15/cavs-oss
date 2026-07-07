# Many-version release stream

10 versions of a 32.00 MiB build; ~3% of blocks change per release.

| Method | Storage | Adjacent updates | v1→vN jump | Any-pair coverage |
|---|---:|---:|---:|---|
| CAVS packfile store | 30.60 MiB (10 packfiles) | 13.70 MiB total | 8.95 MiB | every pair, same objects |
| bsdiff patches | 4.23 MiB (9 adjacent patches) + full artifacts | 4.23 MiB total | 3.60 MiB (dedicated patch) | needs 45 patches (O(N²)) or chain-apply |

CAVS jump v3→v10: 7.56 MiB. Adjacent per release (CAVS): 1.69 MiB, 1.24 MiB, 1.82 MiB, 1.63 MiB, 1.33 MiB, 1.75 MiB, 1.26 MiB, 1.50 MiB, 1.47 MiB.
