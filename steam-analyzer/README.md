# CAVS SteamPipe Analyzer (`cavs-steam`)

**Find Steam update bloat before your players download it.**

Compares two game builds, estimates the update SteamPipe will generate,
detects the pack files that cause oversized updates (reordering, offset
cascades, huge packs) and recommends packaging fixes — **before you publish**.
It does not replace SteamPipe and uploads nothing: it runs entirely locally.

> Estimates are a **predictive model**, not official Steam output. SteamPipe
> splits files into ~1 MiB chunks, compresses and diffs them; the analyzer
> reproduces that model and contrasts it with content-defined chunking
> (FastCDC) to isolate how much of the update comes from chunk misalignment.

## Usage

```sh
cavs-steam compare ./build_v1 ./build_v2 --out report
open report/index.html
```

Produces `report/index.html`, `summary.md`, `results.json` and `files.csv`
with: estimated update size (SteamPipe vs CAVS), the top offending files, why
it happens, and actionable recommendations per engine (Unreal / Unity / Godot).

### CI gate

```sh
cavs-steam ci ./prev_build ./current_build \
  --max-estimated-update 500MiB --max-risk high
```

Exit codes: `0` ok, `2` budget/risk exceeded, `3` tool error. Use it to block a
build in a PR that would ship a disproportionate update.

## What it detects

| Signal | Diagnosis |
|---|---|
| `scattered_changes` | Changes spread across a large pack -> SteamPipe cannot reuse chunks |
| `cdc_reuse_much_higher_than_fixed_reuse` | Content present but misaligned (reordering / offset cascade) |
| `large_pack_file` | Pack > 2 GiB (warn) / > 8 GiB (high) -> heavy client reconstruction |
| `full_rewrite` | Large pack almost entirely rewritten |
| `new_file` | New file (full payload) |

## How it demonstrates the problem (measured)

On a 20 MiB pack:

| Change | SteamPipe update | CAVS (FastCDC) update | Risk |
|---|---:|---:|---:|
| Localized 200 KiB edit | **1.00 MiB** (95% reuse) | 257 KiB | NONE |
| Insert 100 KiB at the start (reorder) | **20.10 MiB** (0% reuse) | 179 KiB | **HIGH** |

The second case is what the tool exists to catch: a real 100 KiB change that,
by shifting every offset, forces Steam to re-download the whole pack — and the
analyzer flags it with the exact cause.

## How it works

Reuses the CAVS engine: FastCDC (`cavs-chunker`) and BLAKE3 (`cavs-hash`).
It memory-maps each file (handles multi-GB packs without loading them into
RAM), computes fixed 1 MiB chunks (SteamPipe model, same-path comparison) and
FastCDC 64 KiB chunks (CAVS model, global reuse), compresses only the new
chunks with zstd-3 to estimate the real payload, and scores per-file risk.

Recommended configuration (why): 1 MiB approximates SteamPipe's documented
behavior; 64 KiB gave the best updates on real games; zstd-3 was the sweet spot
in CAVS's benchmarks.
