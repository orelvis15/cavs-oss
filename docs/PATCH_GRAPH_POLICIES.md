# Patch graph policies — data model and path selection (v1.1.0)

A patch graph is versions (nodes) plus directed old→new patch edges. A
*policy* decides which edges exist. This document describes the model
behind `cavs bench patch-policy` and `cavs patch-policy graph`.

## Data model

`patch_graph.json` (serde JSON, replayable by `simulate`/`explain`):

```text
PatchGraph
  versions[]      id, index, path, size_bytes, compressed_bytes,
                  signature_hash (BLAKE3)
  edges[]         from, to, policies[] (tags), measures[]
  measures[]      engine, raw_patch_bytes, compressed_patch_bytes,
                  diff_ms, apply_ms, verify_ms, peak_rss_mib, verified
  policies[]      name, label, edge_idxs[], notes
  cavs_routes     store_bytes, build_ms, cold[from][to], warm[from][to],
                  install[to]   (absent in structure-only graphs)
```

Edges are deduplicated: `v0→v1` appears once even when adjacent, ladder
and hot-pairs all use it, and is measured once per engine.

## Edge generators

**Adjacent** — `(i, i+1)` for all i. N−1 edges.

**Ladder** — dyadic intervals per level d = 1, 2, 4, …:

- `aligned` (default): level-d edges start at multiples of d
  (`v0→v2, v2→v4, …`). Total stays under 2N; any jump decomposes into
  ≤ ~2·log₂(distance) applies.
- `dense`: every start offset per level (`v1→v3` too). ~N·log N edges,
  shorter chains for unaligned jumps.

**Base hub** — `base→vi` for every i, plus `vi→base` (bidirectional is
the default in the benchmark, because arbitrary old→new jumps need the
reverse edge to route through the hub: old→base→new, 2 steps).
Selection: `first`, `middle`, `latest-major` (last id whose leading
number increased), `fixed:<id>`, or `auto` — candidates are built,
measured, simulated, and the cheapest under the traffic model is
promoted to the `base` policy (the others remain in the graph as
`base-candidate:<id>` for transparency).

**Hot pairs** — the adjacent baseline plus direct `old→latest` edges:
`latest:K` (the K most recent old versions) or `traffic-top:K` (the K
highest-probability non-adjacent pairs). Optionally filtered by a byte
budget ([STORAGE_BUDGET_POLICIES.md](STORAGE_BUDGET_POLICIES.md)).

**All-pairs** — every `(i, j)` with i<j: N·(N−1)/2 edges. Kept strictly
as the theoretical one-hop baseline.

**CAVS** — no edges. Routes come from the content-addressed chunk
inventory (`cavs_routes`), one step for any jump.

## Path selection

For a query `from→to`, the simulator takes the **cheapest path within
the policy's edges**: Dijkstra over (total compressed bytes, steps),
lexicographic — so byte cost decides, and steps break ties. In
structure-only graphs (no measurements) it degrades to fewest steps.
This means every policy is represented by the best it can actually do
with the edges it stored, not by a fixed formula:

- adjacent: the chain `from→from+1→…→to`;
- ladder: the dyadic decomposition (or better, if a cheaper combination
  of stored edges exists);
- base hub: direct edge if the hub is an endpoint, otherwise
  `from→base→to`;
- hot pairs: the direct patch when stored, otherwise the adjacent
  chain fallback;
- all-pairs: always the direct one-hop edge.

Queries with no path (e.g. a one-way base graph asked for `v2→v5`)
fall back to a full compressed download and count against the policy's
**coverage** in the reports.

## Chain risk

Every extra sequential apply increases the failure surface: the
intermediate patch must exist, download and apply cleanly, and a
mid-chain failure leaves more state to recover. `apply_chain_report.md`
reports avg/p95/max steps and apply time per policy, and the route
planner prices the same risk as
`(patch_steps − 1) · STEP_RISK_WEIGHT` ([ROUTE_PLANNER.md](ROUTE_PLANNER.md)).
