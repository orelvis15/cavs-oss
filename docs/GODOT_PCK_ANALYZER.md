# The Godot PCK analyzer — `cavs analyze godot-pck` (v0.9.0)

Godot-specific insight into why a PCK update costs what it costs,
without changing the Godot plugin or your export flow.

```sh
cavs analyze godot-pck old.pck new.pck --out godot-pck-report.md
```

## What it reports

Byte-level (always works, even for unparseable/encrypted PCKs):

- changed windows at 64 KiB and 1 MiB, scatteredness;
- fixed 1 MiB reuse vs content-defined reuse (shifted-content signal);
- sampled entropy (global-compression signal);
- the analyzer findings and Godot-tailored recommendations.

Directory-aware (when the PCK directory is parseable):

- Godot version and pack format (v1 = Godot 3, v2 = Godot 4,
  unencrypted directories);
- total resources, and **the `res://` paths overlapping each changed
  byte range** — so "1.06 MiB of churn" becomes "level02.scn was packed
  in front of everything else".

Unsupported layouts fail soft: the byte-level report still prints, with
`CAVS-E-GODOT-PCK-UNSUPPORTED` explaining why parsing was skipped.

## Measured example (benchmark G)

| Case | SteamPipe-style | CAVS `.cavsplan` |
|---|---:|---:|
| one resource edited in place | 1.00 MiB | 128 KiB |
| new resource packed first (offsets shift) | 3.50 MiB — the whole PCK | 1.06 MiB |

Raw outputs: [results/v0.9.0/steampipe-cases/](results/v0.9.0/steampipe-cases/).

## Recommendations for Godot developers

- **Keep the base PCK stable**; ship frequently updated content in
  separate PCKs.
- **Use resource-pack add-ons for live content**: load update/DLC PCKs
  at runtime with `ProjectSettings.load_resource_pack()` instead of
  rewriting the base PCK (the CAVS Godot plugin mounts packs this way).
- **Keep export order deterministic** so resources keep their offsets
  between exports; avoid repacking unrelated assets.
- **Prefer per-resource compression** over compressing the whole PCK.
- For delivery, `cavs publish-preview old.pck new.pck`-style runs and
  the [Godot plugin](../game-engine-plugins/godot-plugin/README.md) cover the runtime side.

`cavs analyze-packs old.pck new.pck --engine godot` gives the generic
pack table for the same pair
([PACK_FILE_OPTIMIZATION.md](PACK_FILE_OPTIMIZATION.md)).
