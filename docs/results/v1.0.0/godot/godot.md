# Godot Certification

Result: **PASS**

| Check | Result | Details |
|---|---|---|
| old PCK parse | PASS | PCK v4.2.0 (pack format 2), 2 resources |
| new PCK parse | PASS | PCK v4.2.0 (pack format 2), 2 resources |
| .cavsplan output byte-identical | PASS | integrity checks: PASS |
| chunk/hybrid route byte-identical | PASS | CAVS chunk / hybrid (wire) — 72.63 KiB |
| bootstrap route byte-identical | PASS | full zstd-19 (CAVS bootstrap) — 2.50 MiB |
| unverified PCKs never mounted | PASS | every route verifies the reconstructed PCK hash before it is promoted/mounted |
| Godot PCK analyzer report | PASS | godot-pck-analysis.md (actionable layout recommendations) |
| plugin API surface | PASS | fetch, fetch_async and ensure_pack exported — the documented flow stays valid |
| Godot engine smoke test | SKIPPED | pass --godot-bin and --test-project to run it (optional) |

## Certified plugin flow (stable for v1.x)

```gdscript
var cavs := CavsClient.new("http://127.0.0.1:8990")
var result := cavs.fetch("main_pack")   # blocking: run in a Thread
if result.ok:
    ProjectSettings.load_resource_pack(result.files[0])

# or, in one line (fetch + mount the first .pck):
CavsClient.new("http://127.0.0.1:8990").ensure_pack("main_pack")
```

The plugin stays intentionally simple: download only what is needed, reconstruct the PCK, verify the hash, mount it with `ProjectSettings.load_resource_pack()`.
