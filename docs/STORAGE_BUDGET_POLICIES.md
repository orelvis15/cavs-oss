# Storage budget policies — which pairwise patches to store (v1.1.0)

Hot-pair patching is a budget question: given limited patch storage,
which direct old→new patches pay for themselves? The patch policy
benchmark answers it with measurements instead of rules of thumb.

## CLI

```sh
cavs bench patch-policy \
  --versions-dir builds \
  --policies adjacent,hot-pairs,cavs \
  --hot-pairs latest:5 \
  --patch-storage-budget 2x-latest-build \
  --traffic-model live-service-weekly \
  --out results/budget
```

Budgets: absolute sizes (`1GiB`, `500MiB`) or multiples of the latest
compressed build (`2x-latest-build`, `0.5x-latest-build`).

## Candidate selection

`--hot-pairs latest:K` proposes direct `old→latest` patches for the K
most recent old versions; `traffic-top:K` proposes the K
highest-probability non-adjacent jumps from the traffic model. The
adjacent baseline is always stored (it is the fallback chain), so the
budget applies to the direct hot patches on top of it.

## Greedy selection

Every candidate is actually diffed and measured, then scored:

```text
score(edge) = expected_traffic × (fallback_route_bytes − patch_bytes)
              ───────────────────────────────────────────────────────
                              patch_bytes
```

— expected bytes saved per stored byte, where `fallback_route_bytes`
is what the client would download without the patch (the adjacent
chain, or a full download when no chain exists). Candidates are taken
best-first while they fit the budget, and a patch is stored **only if
it beats its fallback route at all** — a patch larger than the chain it
replaces is never kept, budget or not.

`storage_report.md` lists every candidate with its patch size, fallback
size, traffic share and kept/rejected verdict, so the selection is
auditable.

## Relationship to `.cavspatch` sidecars

This is the same stance as CAVS's own sidecar policy
([PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md)): pairwise patches are an
*optimization for hot pairs*, never a coverage requirement, because the
content-addressed store already serves every jump. The benchmark's
`hybrid-cavs-fallback` reading: compare `hot-pairs` and `cavs` rows in
`summary.md` — a hot pair is only worth storing where it beats the CAVS
route by more than its storage cost.
