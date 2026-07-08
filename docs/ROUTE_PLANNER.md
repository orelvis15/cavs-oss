# The policy route planner — `cavs plan-update` (v0.9.0)

`cavs route-plan` (v0.8.0, [DELIVERY_PLANNER.md](DELIVERY_PLANNER.md))
picks the smallest viable route under a device profile. v0.9.0 adds
`cavs plan-update`: the same route set scored under **explicit
policies** — because the best route is not always the smallest
download.

```sh
cavs plan-update \
  --from ./installed_v1 --to ./build_v2 \
  --client-state cold-cache,has-previous-install \
  --routes all --policy balanced
```

## Routes considered

| Route | Needs | Cost source |
|---|---|---|
| full download | — | exact (raw size) |
| bootstrap | — | exact with `--bootstrap`, else estimated (zstd-3) |
| chunk route | warm cache | exact (fresh chunk bytes) |
| hybrid previous-artifact | previous install | exact (fresh chunk bytes) |
| `.cavsplan` | previous install | exact (built on the spot, or `--plan`) |
| optimized sidecar | `--patch` file | exact |
| butler offline | measured externally | reported unavailable here |
| bsdiff / xdelta3 proxies | measured externally | reported unavailable here |

**A missing route is never chosen.** Routes that need artifacts nobody
generated (sidecars are hot-pair-only), tools that are not installed,
or state the client does not have (cold cache disables the chunk
route) appear in the table as `[unavailable]` with the reason.

## Client state

`--client-state` is a comma list: `warm-cache` / `cold-cache`,
`has-previous-install` / `cold-install`, `low-ram` (128 MiB apply
budget — excludes high-memory routes), `low-disk` (temporary disk
heavily penalized), `slow-hdd` (full-file copies and old-install reads
penalized 5×), `fast-nvme`.

## Policies

```text
score = network_MiB · w_net + apply_ms · w_apply + ram_MiB · w_ram
      + temp_disk_MiB · w_temp + disk_read_MiB · w_read + build_ms · w_build
      + (patch_steps − 1) · step_risk_weight
```

The last term (v1.1.0) prices patch-chain risk: every sequential patch
apply beyond the first adds a fixed penalty (`STEP_RISK_WEIGHT`, 25
score points ≈ 2.5 MiB under `balanced`), because a chain multiplies
the failure surface — each intermediate patch must exist, download and
apply cleanly, and a failure mid-chain leaves more state to recover.
All built-in routes are single-step; the penalty exists so a
graph-fed route (an adjacent or ladder patch chain from
[PATCH_GRAPH_POLICIES.md](PATCH_GRAPH_POLICIES.md)) is never chosen
over a one-step route just because it saves a few KiB. The same risk
model is what `cavs bench patch-policy` reports per policy in
`apply_chain_report.md`.

| Policy | Optimizes for |
|---|---|
| `network_min` | smallest download, everything else a tiebreak |
| `cpu_min` | cheap apply (full/chunk routes win more often) |
| `ram_min` | avoid bsdiff-class high-memory applies |
| `disk_io_min` | avoid full pack rebuilds and staging |
| `balanced` | network-first with sane secondary weights (default) |
| `hdd_friendly` | heavily penalize seeks and full-file copies |
| `developer_fast` | minimize diff/build time on the publisher side |

## Output

Human table (score-sorted, unavailable routes annotated) or `--json`.
The choice always comes with the reason:

```text
chosen  : .cavsplan — 128.81 KiB over the wire · ~40.00 MiB peak RAM ·
          64.00 MiB temp disk · policy balanced
```

Measured decisions across client states (benchmark H):
[results/v0.9.0/route-planner/](results/v0.9.0/route-planner/). Note
the honest cold-install case: on incompressible data the planner picks
the raw full download over a bootstrap that saves nothing.

## Integration

`cavs publish-preview` uses the same measured routes on the publisher
side; `cavs install-plan` applies per-depot route suggestions inside a
workspace ([DEPOTS_BRANCHES_WORKSPACE.md](DEPOTS_BRANCHES_WORKSPACE.md)).
