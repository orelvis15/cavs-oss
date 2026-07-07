# Route selection and its certification (v1.0.0)

CAVS is multi-route: the same old→new transition can be served as a
full download, a bootstrap artifact, chunk/hybrid delivery, an offline
`.cavsplan`, an optimized pairwise sidecar — and compared against
butler offline and bsdiff/xdelta3 proxies when those tools are
installed. The planner (`cavs plan-update`, [DELIVERY_PLANNER.md](DELIVERY_PLANNER.md))
scores the available routes for one client state under a policy;
`cavs certify routes` certifies those decisions.

## Client states certified by default

| State | Meaning | Planner tokens |
|---|---|---|
| `cold-install` | no previous install, no cache | `cold-install` |
| `cold-cache-previous` | previous install exists, CAVS cache empty | `cold-cache,has-previous-install` |
| `warm-cache` | previous CAVS cache exists | `warm-cache,has-previous-install` |
| `exact-previous-version` | exact old version installed | `has-previous-install` |
| `low-ram` | avoid high-RAM bsdiff-like routes | `has-previous-install,low-ram` |
| `slow-hdd` | avoid high disk-I/O routes | `has-previous-install,slow-hdd` |
| `limited-disk` | minimize temporary disk usage | `has-previous-install,low-disk` |

Override with `--client-states` (documented names or raw planner
tokens). Workspace-specific states — demo-to-full, DLC ownership,
language packs — are certified by `cavs certify workspace` through its
install plans instead.

## Selection rules (certified)

1. A route whose dependency is unavailable is never chosen — missing
   optional tools/artifacts mark the route unavailable, and the report
   lists them as *skipped*, not selected.
2. A route that fails output verification is never recommended, and any
   measured route with a non-identical output fails the certification.
3. A route that exceeds the policy limits (RAM budget, disk pressure)
   is scored out.
4. Near-ties (within 2% network) prefer the simpler, lower-risk route —
   `.cavsplan` over exotic alternatives.
5. A cold install must never depend on a previous install: certification
   fails if the planner picks a plan/hybrid/no-op route for it.

## Policies and scores

`--policy` selects the scoring weights (default `balanced`; also
`network_min`, `cpu_min`, `ram_min`, `disk_io_min`, `hdd_friendly`,
`developer_fast`). `routes.json` records the weights and the per-route
scores so a decision is always explainable:

```json
{
  "schema": "cavs-certify-routes/1",
  "policy": "balanced",
  "weights": { "network": …, "apply_ms": …, "ram_mb": … },
  "states": [
    {
      "state": "exact-previous-version",
      "chosen": "offline plan (.cavsplan)",
      "reason": "…",
      "network_bytes": 2373513,
      "routes": [ { "route": "…", "score": …, "available": true, … } ]
    }
  ],
  "measured": { "routes": [ … ] },
  "recommended": "CAVS offline plan (.cavsplan)",
  "metrics": { "network_bytes": …, "apply_ms": …, "peak_ram_bytes": … }
}
```

## Estimates vs measurements

- `--routes estimate` (and the `quick` profile) runs the planner only:
  fast, no applies.
- `--routes all` (default from `standard` up) additionally runs the
  measured matrix from `cavs bench routes`: real subprocess applies,
  byte-identical verification per route, peak RSS, and butler /
  bsdiff / xdelta3 when installed. The raw rows land in
  `artifacts/route-results.json`.

The recommended route is the smallest **verified** network payload,
with `.cavsplan` breaking near-ties — the same rule as
`cavs publish-preview`.
