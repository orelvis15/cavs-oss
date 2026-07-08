# Patch policy benchmark results (v1.1.0)

These files are the output of `cavs bench patch-policy` on a
deterministic 10-version release stream (32 MiB per version, ~3% of
64 KiB blocks change per release). Every pairwise number is a real diff,
applied and byte-verified. Full harness docs:
[../../../PATCH_POLICY_BENCHMARK.md](../../../PATCH_POLICY_BENCHMARK.md).

The comparison is deliberately framed as *practical pairwise patch
policies* — adjacent, sparse ladder, base hub, hot pairs — plus the
all-pairs graph kept only as the theoretical one-hop baseline, and the
CAVS content-addressed route. It is not a "CAVS beats pairwise" claim:
adjacent diffs win storage and per-update bytes for users who update
every release; the tables show where each policy is the better tradeoff.

## How to reproduce

```sh
# 1. deterministic dataset (same seed + size ⇒ identical bytes)
cavs bench gen-stream --out builds --versions 10 --size 32MiB --seed 5

# 2. measure every policy under a traffic model (bsdiff/xdelta3 columns
#    appear only when those tools are on PATH; missing ones are skipped,
#    never fatal — cavsplan is built in)
cavs bench patch-policy \
  --versions-dir builds --version-glob 'v*' --sort semver \
  --policies adjacent,ladder,base,hot-pairs,all-pairs,cavs \
  --patch-engines cavsplan,bsdiff,xdelta3 \
  --traffic-model adjacent-heavy \
  --hot-pairs latest:3 --patch-storage-budget 2x-latest-build \
  --out docs/results/v1.1.0/patch-policy

# 3. replay other scenarios on the measured graph — no re-diffing
cavs patch-policy simulate --graph patch_graph.json --traffic-model skip-heavy
cavs patch-policy simulate --graph patch_graph.json --client-state warm-cache
cavs patch-policy explain  --graph patch_graph.json --from v01 --to v10 --policy ladder
```

## What each file is

| File | What it holds |
|---|---|
| `summary.md` | The headline table: patch count, storage, avg/p95/p99 update bytes, max steps, build time, coverage per policy (adjacent-heavy, cold cache + previous install). |
| `summary.json` | Machine-readable version of `summary.md` — every percentile, plus the version list, traffic model, engine and client state. |
| `summary-skip-heavy.md` | Same graph replayed under skip-heavy traffic (40% adjacent, 40% skip 2–8). |
| `summary-warm-cache.md` | Same graph replayed with a warm chunk cache (CAVS route seeded by earlier updates). |
| `patch_graph.json` | The full measured graph — versions, edges and per-engine measurements. Replayable by `simulate`/`explain` with **no re-diffing**. |
| `policy_edges.csv` | One row per edge × engine (see columns below). |
| `query_results.csv` | One row per traffic query × policy — what each update actually cost. |
| `storage_report.md` | Storage vs latest build size, total bytes served, and the hot-pair budget selection (which candidates were kept and why). |
| `traffic_report.md` | The expanded weighted query distribution the averages are computed over. |
| `apply_chain_report.md` | Avg/p95/max apply steps and apply time per policy — the chain-length / failure-surface view. |
| `tool_versions.json` | Exact tool versions used for the run. |

## CSV columns

**`policy_edges.csv`** — every measured patch edge:

```
from, to                    version ids (old → new)
policies                    which policies use this edge (+-joined)
engine                      cavsplan | bsdiff | xdelta3 | butler-offline
raw_patch_bytes             patch size as the engine emitted it
compressed_patch_bytes      patch size after zstd recompression (min of the two)
diff_ms, apply_ms, verify_ms   measured times for this edge
peak_rss_mib                peak memory during diff/apply (blank if unmeasured)
verified                    true only if the applied output matched the target byte-for-byte
```

**`query_results.csv`** — every user update priced under every policy:

```
policy                      adjacent | ladder | base | hot-pairs | all-pairs | cavs
from, to                    the jump the user makes
rule                        which traffic rule generated it (adjacent, skip_range, old_to_latest, reinstall_latest)
probability                 this query's share of total traffic (weights the averages)
bytes                       bytes served for this jump under this policy
steps                       sequential patch applies (1 = one hop; 0 = full download / reinstall)
apply_ms, verify_ms         summed over the chain
covered                     false ⇒ the policy had no path and a full download was served
```

## Reading the numbers honestly

- **Adjacent** is the storage and per-update winner when users update
  every release — its design point. The cost is chain length on skips.
- **Ladder** is the strongest practical pairwise baseline here: near
  adjacent bytes with chains bounded at a few steps.
- **All-pairs** is the byte/steps optimum and the storage/build
  pathology; it is labeled the theoretical one-hop baseline, never
  "pairwise diffs".
- **CAVS** trades per-pair bytes (a single exact patch is smaller) for
  one apply step on any jump, no patch graph to host, cache/hybrid
  reuse and reinstalls from the same store.
