# File formats — index (v1.0.0)

Every on-disk format CAVS reads or writes, its stability status for
v1.x, and where the byte-level specification lives. The compatibility
rules (versioned readers, additive JSON, deprecation policy) are in
[COMPATIBILITY.md](COMPATIBILITY.md).

## Binary formats (stable)

| Extension | Magic | What it is | Specification |
|---|---|---|---|
| `.cavs` | `CAVS` | Deduplicated content container: FastCDC/fixed chunks, zstd, BLAKE3 Merkle, optional Ed25519 content signature | [FORMAT.md](FORMAT.md) |
| `.cavsmf2` | `CAVSMF2` | Compact binary manifest (v2) negotiated by server/client | [FORMAT.md](FORMAT.md) |
| `.cavssig` | `CAVSSIG1` | Old-version signature: layout + weak/strong block hashes, enough to plan an update without the old bytes | [SIGNATURE_FORMAT.md](SIGNATURE_FORMAT.md) |
| `.cavsplan` | `CAVSPLAN1` | Deterministic offline reconstruction plan (copy-old + inline ops, zstd payload, BLAKE3-sealed) | [CAVSPLAN_FORMAT.md](CAVSPLAN_FORMAT.md) |
| `.cavspatch` | `CAVSPCH1` / v2 | Optimized pairwise sidecar with per-file strategy selection | [PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md) |
| `.cavs.bootstrap.zst` | zstd frame | Whole-artifact bootstrap for cache-less installs | [FORMAT.md](FORMAT.md) |
| `.cavspack` | store layout | Immutable packfiles of the global store | [FORMAT.md](FORMAT.md) |

## JSON schemas (stable, additive)

Every machine-readable report carries a `schema` field:

| Schema | Produced by |
|---|---|
| `cavs-certify-summary/1` | `cavs certify` (`summary.json`, `--json-out`) |
| `cavs-certify-integrity/1` | `cavs certify integrity` |
| `cavs-certify-routes/1` | `cavs certify routes` |
| `cavs-certify-regressions/1` | `cavs certify regressions` |
| `cavs-certify-godot/1` | `cavs certify godot` |
| `cavs-certify-workspace/1` | `cavs certify workspace` |
| `cavs-certify-baseline/1` | `--save-baseline` |

Reports without a `schema` field (bench outputs, preview JSON) are
informational and may evolve between minor versions.

## Workspace metadata

`cavs workspace` stores TOML/JSON metadata under the workspace
directory (apps, depots, branches, builds, depot indices). The layout
is documented in [DEPOTS_BRANCHES_WORKSPACE.md](DEPOTS_BRANCHES_WORKSPACE.md);
it is stable for v1.x with the same additive rules as the JSON schemas.

## Repro bundles

`repro.tar.zst` — a deterministic tar (sorted entries, zeroed
timestamps/owners) compressed with zstd-19; contents documented in
[REPRODUCIBILITY.md](REPRODUCIBILITY.md).
