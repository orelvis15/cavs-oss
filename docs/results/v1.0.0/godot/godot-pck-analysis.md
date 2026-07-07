# Godot PCK Analysis

> SteamPipe-style estimate based on public documentation. This is not Valve's exact SteamPipe implementation.

`steampipe-cases/datasets/godot-pck-localized/old/game.pck` → `steampipe-cases/datasets/godot-pck-localized/new/game.pck`

| Metric | Value |
|---|---:|
| Old size | 2.50 MiB |
| New size | 2.50 MiB |
| Changed 64 KiB windows | 2 |
| Changed 1 MiB windows | 1 |
| Scatteredness | 0.00 |
| Entropy | 8.00 bits/byte |
| Fixed 1 MiB reuse | 66.7% |
| Content-defined reuse | 97.1% |
| PCK directory | parsed (Godot 4.2.0 (pack format 2), 2 resources) |

## Resources overlapping changed regions

- `res://textures/hero.png`

## Recommendations

- The PCK behaves like a compressed blob; prefer per-resource compression so small changes stay local.
- Content survives but offsets moved — keep export order deterministic so resources keep their positions between exports.
- Keep the base PCK stable; ship frequently updated content in separate PCKs.
- Load update/DLC PCKs as resource packs at runtime instead of rewriting the base PCK.
- Avoid repacking unrelated assets: unchanged resources should keep their offsets.

## Findings

- **[info]** Godot: split the base PCK from update PCKs
