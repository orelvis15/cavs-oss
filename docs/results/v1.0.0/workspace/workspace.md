# Workspace Certification

Result: **PASS**

| Check | Result | Details |
|---|---|---|
| metadata parse | PASS | workspace.toml valid |
| app exists | PASS | app 'my-game' |
| depots exist | PASS | 5: base, dlc1, windows, linux, lang-es |
| branches valid | PASS | 1 branches, every reference resolves |
| from build | PASS | 'build_1001' → build_1001 (5 depots) |
| to build | PASS | 'build_1002' → build_1002 (5 depots) |
| build depot indices | PASS | 5 indices load |
| branch promote preview | PASS | branch 'beta' → build build_1002 |
| rollback preview | PASS | branch 'beta' can roll back to previously-served build 'build_1001' |
| depot sharing | PASS | 10 pairs, deterministic; depot-sharing.md |
| per-depot update cost | PASS | base: 9.25 MiB, windows: 0 B, linux: 0 B, lang-es: 0 B, dlc1: 0 B |
| install-plan linux + base | PASS | install-plans/linux--base.md |
| install-plan windows + base | PASS | install-plans/windows--base.md |
| install-plan linux + es + lang-es | PASS | install-plans/linux--es--lang-es.md |
| install-plan linux + base + dlc1 | PASS | install-plans/linux--base--dlc1.md |

## Per-depot update cost

| Depot | Update | Depot total |
|---|---:|---:|
| base | 9.25 MiB | 126.89 MiB |
| windows | 0 B | 3.81 MiB |
| linux | 0 B | 3.81 MiB |
| lang-es | 0 B | 292.97 KiB |
| dlc1 | 0 B | 7.63 MiB |
