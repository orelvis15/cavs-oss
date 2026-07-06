# Offline Toolkit (v0.7.0)

CAVS v0.7.0 turns the delivery system into a complete **local toolkit**:
sign, preview, diff, apply, verify and benchmark game-build updates with
no CAVS server involved. The same reconstruction model the online client
uses (verified copy-ranges + fresh data) drives every offline command.

```bash
# 1. Describe the released version (once, at release time):
cavs signature export ./Build_v1 --raw -o build_v1.cavssig

# 2. See what the next build changes before publishing anything:
cavs preview ./Build_v2 --against build_v1.cavssig --changes-only

# 3. Produce a deterministic offline update plan (a portable patch):
cavs diff-plan ./Build_v1 ./Build_v2 -o update.cavsplan --report plan.md

# 4. Apply it — staged, journaled, verified, mod-friendly:
cavs apply --old ./InstalledGame --plan update.cavsplan --inplace --verify

# 5. Check any install against a known-good state:
cavs verify-install ./InstalledGame --signature build_v2.cavssig --allow-extra-files

# 6. Identify and inspect any CAVS file:
cavs file update.cavsplan
cavs ls build_v1.cavssig
```

Every command supports `--json` where output is meant to be consumed
programmatically.

## `cavs preview`

Classifies every entry of the new build against the old `.cavssig` as
`NEW` / `MODIFIED` / `DELETED` / `SAME`, estimates the update cost per
route, and warns when a large modified file looks compressed/high-entropy
(small source changes cascade across compressed output — publish the
uncompressed folder instead).

```text
MODIFIED    19.25 MiB  game.pck
NEW        320.00 KiB  assets/asset_40.dat
DELETED          0 B   assets/asset_07.dat

Summary:
  estimated CAVS update    : 717.31 KiB (fresh 1.58 MiB, block-level reuse 32.19 MiB)
  estimated full zstd-3    : 15.59 MiB
```

## `cavs diff-plan` and the `.cavsplan`

A plan is a deterministic, BLAKE3-sealed description of how to rebuild the
new build from the old one: COPY ranges that reuse old bytes, INLINE data
(zstd-19) for what changed, plus directory metadata (created dirs,
symlinks, executable bits, managed deletions). Two kinds:

- **portable** (default): carries the inline payload — a self-contained
  offline patch;
- **analysis** (`--analysis`): ops and estimates only, for reports and CI
  size gates.

`--old-signature build_v1.cavssig` diffs without the old bytes present —
only the new build and the previous release's signature are needed.
Format details: [CAVSPLAN_FORMAT.md](CAVSPLAN_FORMAT.md).

## `cavs apply`

- **Artifact plans**: write `<out>.part`, verify the full BLAKE3, then
  atomically rename. A wrong or corrupted old artifact aborts with
  `CAVS-E-APPLY-HASH-MISMATCH` and leaves nothing behind.
- **Directory plans**: reconstruct changed files into
  `<out>/.cavs-staging/`, verify every file hash *before* commit, write a
  journal (`.cavs-journal.json`), then commit with per-file renames.
  An interrupted apply is finished by re-running the same command (or
  `cavs apply --resume <journal>`); a journal from a *different* apply
  blocks with `CAVS-E-JOURNAL-RESUME-FAILED` instead of guessing.

Mod-friendly by default:

- files whose hash already matches are **never touched** (mtime survives);
- files the plan does not manage (mods, saves) are **preserved**;
- managed deletions happen only with `--delete-removed-files`.

## `cavs verify-install`

Verifies an installed artifact or directory against a `.cavssig` (block
hashes) or a manifest (`sha256:` digests), reporting the exact mismatch
type per entry — `MODIFIED` / `MISSING` / `EXTRA` — and exiting non-zero
on failure. `--allow-extra-files` tolerates mods and saves.

## `cavs file` / `cavs ls`

Identify any CAVS file by magic — `.cavs` containers, `.cavssig`
signatures, `.cavsplan` plans, `.cavspatch` sidecars, manifests, zstd
bootstraps — and list what is inside. Unknown or corrupt files fail
cleanly with a non-zero exit.

## Measured (128 MiB synthetic builds, seed 5)

| Update | Full zstd-19 | CAVS `.cavsplan` | Apply time |
|---|---:|---:|---:|
| directory build, typical change | 62.12 MiB | **2.51 MiB** | 264 ms |
| single artifact, small change | 64.05 MiB | **1.94 MiB** | 95 ms |
| shifted artifact (all bytes moved) | 64.06 MiB | **4.21 KiB** | 94 ms |

Full route comparisons (including butler and bsdiff/xdelta3):
[ROUTE_BENCHMARKS.md](ROUTE_BENCHMARKS.md) and
[BUTLER_COMPARISON.md](BUTLER_COMPARISON.md).

## Error taxonomy additions (v0.7.0)

| Code | Meaning |
|---|---|
| `CAVS-E-PLAN-CORRUPT` | `.cavsplan` unparseable / integrity failure |
| `CAVS-E-PLAN-INVALID` | plan parsed but is internally inconsistent |
| `CAVS-E-APPLY-HASH-MISMATCH` | output hash wrong; nothing committed |
| `CAVS-E-JOURNAL-CORRUPT` | apply journal unreadable |
| `CAVS-E-JOURNAL-RESUME-FAILED` | journal belongs to a different apply |
| `CAVS-E-PATH-TRAVERSAL` | container path escapes its root |
| `CAVS-E-UNSUPPORTED-SYMLINK` | symlink not representable on this platform |
| `CAVS-E-PAIRWISE-TOOL-MISSING` | bsdiff/xdelta3/brotli not available |
| `CAVS-E-BUTLER-NOT-FOUND` / `-DIFF-FAILED` / `-APPLY-FAILED` / `-VERIFY-FAILED` | external butler benchmark harness |
