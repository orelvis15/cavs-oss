# Practical pairwise diff policies (v1.1.0)

Pairwise diffs are not one strategy. "Pairwise patches need O(N²)
storage" is only true for the **all-pairs, one-hop** graph — the case
where every old→new jump must be served as a single direct patch. Real
patch systems almost never deploy that graph. They deploy one of the
policies below, each a different tradeoff between patch storage, chain
length, apply cost and build cost.

CAVS does not dismiss pairwise diffs. `cavs bench patch-policy`
measures these policies head-to-head — real diffs, real applies, real
verification — against CAVS content-addressed routes, under an explicit
user traffic model ([TRAFFIC_MODELS.md](TRAFFIC_MODELS.md)).

## Adjacent diffs

Store only `v0→v1, v1→v2, …` — O(N) patches.

Best for users who update every release: the adjacent patch is usually
the smallest possible download for that jump. The cost appears when
users skip versions: a `v2→v6` update chain-applies four patches, every
intermediate patch must exist and apply cleanly, and old clients pay
long sequential chains.

## Sparse power-of-two ladder diffs

Store patches over dyadic intervals (`v0→v1…`, `v0→v2, v2→v4…`,
`v0→v4…`) — fewer than 2N patches, and any jump decomposes into
O(log distance) applies.

Good for skipped versions at a small storage premium over adjacent-only.
Large-distance ladder patches can be big, and the graph is more complex.
`--ladder-mode aligned` (default) keeps the canonical <2N aligned
intervals; `dense` adds unaligned starts for shorter chains at ~N·log N
storage.

## Base-version diffs

Keep a base version and store diffs around it (`base→vi`, optionally
`vi→base`) — O(N) one-way, O(2N) bidirectional.

Simple to reason about and good when most installs sit near a known
base (a major release). Arbitrary old→new jumps route through the hub
in two steps, base diffs grow as content drifts from the base, and a
bad base choice hurts every query. `--base-policy auto` tests
candidates and keeps the best under the traffic model.

## Hot-pair diffs

Store direct patches only for the transitions expected to be common —
`latest:K`, traffic-driven pairs, or a byte budget
([STORAGE_BUDGET_POLICIES.md](STORAGE_BUDGET_POLICIES.md)) — with a
fallback (chain or CAVS route) for everything else.

Matches real traffic when telemetry exists; wrong assumptions waste
storage. This is also exactly how CAVS deploys its own `.cavspatch`
sidecars ([PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md)).

## All-pairs

Every old→newer pair directly: O(N²) patches, one-hop everything.

Best possible direct patch size per exact pair, and the benchmark keeps
it for exactly that reason — as the **theoretical one-hop baseline**.
It is not labeled "pairwise diffs" in any CAVS report, because that
would misrepresent the practical systems above.

## How CAVS compares

CAVS uses a content-addressed, cache-aware route planner: persistent
chunk cache, previous-install reconstruction, `.cavsplan` offline
plans, bootstrap fallback — and it can *also* use selected pairwise
sidecars for hot pairs. A single exact pairwise patch may be smaller in
bytes for one pair; CAVS trades that for serving arbitrary jumps with
no patch graph, one apply step, cross-version cache reuse and verified
reconstruction.

The best policy depends on user update behavior, patch storage budget,
build frequency, apply cost, memory, disk I/O, and how often users skip
versions. That is a measurement, not an argument — run:

```sh
cavs bench patch-policy --versions-dir builds \
  --policies adjacent,ladder,base,hot-pairs,all-pairs,cavs \
  --traffic-model skip-heavy --out results/patch-policy
```

See [PATCH_POLICY_BENCHMARK.md](PATCH_POLICY_BENCHMARK.md) for the full
harness and [BENCHMARKS.md](BENCHMARKS.md) for measured results.
