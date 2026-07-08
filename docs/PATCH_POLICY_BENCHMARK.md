# The patch policy benchmark — `cavs bench patch-policy` (v1.1.0)

Given a sequence of builds `v0…vN`, the benchmark answers:

- How much storage does each patch policy require?
- How much build time does each policy require?
- How many patches must be applied for common user update paths?
- How many bytes are served on average, at p95/p99, and worst case?
- What happens when users skip versions, or return after many releases?
- When does CAVS beat practical pairwise policies — and when do
  adjacent or ladder policies beat CAVS?

Every pairwise number is a **real measurement**: the diff is generated,
applied and byte-verified against the target build. Missing external
tools skip that engine with a recorded reason; they never fail the run.

## Quick start

```sh
# deterministic dataset: 10 versions, ~3% drift per release
cavs bench gen-stream --out builds --versions 10 --size 32MiB

cavs bench patch-policy \
  --versions-dir builds --version-glob 'v*' --sort semver \
  --policies adjacent,ladder,base,hot-pairs,all-pairs,cavs \
  --patch-engines cavsplan,bsdiff,xdelta3 \
  --traffic-model adjacent-heavy \
  --out results/patch-policy
```

Versions can also be listed explicitly (`--versions ./b/v0 ./b/v1 …`)
and can be single artifacts or directory builds (bsdiff/xdelta3 measure
single artifacts only; `cavsplan` and `butler-offline` handle both).

## Policies

| Policy | Patch count | Best use case |
|---|---:|---|
| `adjacent` | N−1 | users update every version |
| `ladder` | <2N (aligned) | skipped versions, bounded chains |
| `base` | 2(N−1) bidirectional hub | major-baseline workflows |
| `hot-pairs` | adjacent + budgeted | traffic-driven optimization |
| `all-pairs` | N·(N−1)/2 | **theoretical one-hop baseline only** |
| `cavs` | content store (no graph) | route-planned cache/hybrid updates |

Details and tradeoffs: [PATCH_GRAPH_POLICIES.md](PATCH_GRAPH_POLICIES.md)
and [PRACTICAL_PAIRWISE_DIFFS.md](PRACTICAL_PAIRWISE_DIFFS.md).
All-pairs is always labeled *theoretical one-hop baseline* — never
"pairwise diffs" — because it is not how pairwise systems deploy.

Key options: `--ladder-mode aligned|dense`, `--base-policy
first|middle|latest-major|fixed:<id>|auto` (auto tests candidates and
keeps the best under the traffic model), `--hot-pairs latest:K |
traffic-top:K`, `--patch-storage-budget 1GiB|2x-latest-build`
([STORAGE_BUDGET_POLICIES.md](STORAGE_BUDGET_POLICIES.md)).

## Engines

`--patch-engines cavsplan,bsdiff,xdelta3,butler-offline`. Each edge is
measured per engine: diff time, patch size (raw + recompressed),
apply time, verify time, peak RSS where the platform reports it.
`cavsplan` is built in, so the benchmark always completes; summary
tables use the first engine that produced a verified measurement for
every edge, and `policy_edges.csv` has the per-engine detail.

## Traffic models and client states

`--traffic-model adjacent-heavy|skip-heavy|live-service-weekly|
major-release|random|custom:file.toml` — how users actually move
between versions ([TRAFFIC_MODELS.md](TRAFFIC_MODELS.md)).
`--client-state cold-cache-with-previous-install` (default) or
`warm-cache` selects which CAVS route is priced. Queries a policy
cannot serve fall back to a full compressed download and count against
its coverage.

The CAVS row is priced from the chunk inventory: cold = compressed
chunks of the target the previous install doesn't have; warm = chunks
the accumulated cache doesn't have; reinstalls = full chunk install
from the store. Always one apply step, no per-pair generation.

## Outputs

A committed example run with a short guide to every file lives in
[results/v1.1.0/patch-policy/](results/v1.1.0/patch-policy/) (start with
its `README.md`).

```text
results/patch-policy/
  summary.md              policy comparison table (avg/p95/p99, steps, coverage)
  summary.json            machine-readable summaries
  patch_graph.json        versions + edges + measurements (replayable)
  policy_edges.csv        every edge × engine measurement
  query_results.csv       every traffic query × policy outcome
  storage_report.md       storage vs latest build, budget selection
  traffic_report.md       the expanded query distribution
  apply_chain_report.md   steps and apply-time risk per policy
  tool_versions.json      exact tool versions used
  raw/<engine>/…          patch artifacts (with --keep-patches)
```

## Replaying and inspecting

```sh
# different traffic on the same measured graph — no re-diffing
cavs patch-policy simulate --graph results/patch-policy/patch_graph.json \
  --traffic-model skip-heavy --client-state warm-cache

# the exact path one policy takes for one query
cavs patch-policy explain --graph results/patch-policy/patch_graph.json \
  --from v01 --to v09 --policy ladder

# structure-only graph (no diffs) for planning
cavs patch-policy graph --versions-dir builds --policies adjacent,ladder \
  --out graph.json
```

`explain` output:

```text
sparse dyadic ladder (aligned) path v01→v09:
  v01→v02  412 KiB
  v02→v04  870 KiB
  v04→v08  1.2 MiB
  v08→v09  390 KiB

Total:
  2.83 MiB
  4 steps
```

## The stance

CAVS is not claiming pairwise patching is bad. CAVS measures pairwise
patch policies and chooses where its content-addressed model is a
better tradeoff. Adjacent diffs often give excellent download sizes for
users who update every version; CAVS offers a different tradeoff —
cache reuse, previous-install reconstruction, route planning, and
arbitrary-version updates without a patch graph. The benchmark exists
so that comparison is explicit, measurable and fair.

Measured results: [BENCHMARKS.md](BENCHMARKS.md#pairwise-patch-policy-benchmark)
and [results/v1.1.0/patch-policy/](results/v1.1.0/patch-policy/).
