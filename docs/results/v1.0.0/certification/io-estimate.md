# Local Disk I/O Estimate

> estimates from the SteamPipe-style analysis model

`dataset/Build_v1` → `dataset/Build_v2`

| Route | Download | Read old | Write | Temp required | Creates | Renames | Deletes | hdd est. | nvme est. | sata_ssd est. |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| full download (raw) | 126.89 MiB | 0 B | 0 B | 126.89 MiB | 44 | 44 | 2 | 1.6s | 52ms | 286ms |
| SteamPipe-style (fixed 1 MiB) | 16.90 MiB | 77.95 MiB | 81.14 MiB | 81.14 MiB | 3 | 6 | 2 | 1.9s | 62ms | 377ms |
| CAVS chunks / hybrid | 4.42 MiB | 77.95 MiB | 81.14 MiB | 81.14 MiB | 3 | 6 | 2 | 1.8s | 57ms | 350ms |
| CAVS .cavsplan | 4.42 MiB | 77.95 MiB | 81.14 MiB | 76.81 MiB | 3 | 6 | 2 | 1.8s | 57ms | 350ms |

> **SteamPipe-style (fixed 1 MiB)**: local I/O (77.95 MiB read + 81.14 MiB write) exceeds the whole build — the network saving does not translate into a faster update on slow disks.

> **CAVS chunks / hybrid**: local I/O (77.95 MiB read + 81.14 MiB write) exceeds the whole build — the network saving does not translate into a faster update on slow disks.

> **CAVS .cavsplan**: local I/O (77.95 MiB read + 81.14 MiB write) exceeds the whole build — the network saving does not translate into a faster update on slow disks.
