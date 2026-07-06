# butler comparison (v0.7.0)

CAVS uses **butler offline as an external benchmark**. The
`cavs bench butler-offline` harness does not reimplement anything: it
drives a local `butler` binary (`-j` JSON-lines mode) through its real
`diff` → `apply` → `verify` pipeline, captures the raw output, measures
wall time and peak RSS, and independently verifies the applied output
byte-for-byte.

## What is (and is not) being measured

butler's pipeline has two patching phases:

1. `butler push` computes a **default patch locally** with an rsync-style
   fixed-block algorithm and low-quality Brotli — optimized for fast
   uploads and low memory.
2. The itch.io **backend later regenerates an optimized patch** (bsdiff +
   high-quality Brotli) that replaces the default one for players.

`butler diff` offline measures phase 1 only. Therefore:

- **butler offline/default patch is not necessarily the same as itch.io's
  backend-optimized patch**, and results here are always labeled as the
  offline/default path.
- CAVS also reports **bsdiff/Brotli optimized pairwise proxy** results
  (`cavs bench pairwise-proxy`) to approximate the optimized class — and
  those are labeled as **proxy results, not official itch.io backend
  results**.

## Measured (butler v15.27.0, 126 MiB synthetic directory build)

| Metric | butler offline | CAVS offline plan |
|---|---:|---:|
| patch size | 2.52 MiB (+61.7 KiB signature) | **2.51 MiB** (signature embedded) |
| diff time | 983 ms | **488 ms** |
| apply time | 348 ms | **262 ms** |
| peak RSS | 35 MiB | streaming (8 MiB read budget) |
| output byte-identical | yes | yes |

Same transition, single 128 MiB artifact with a small change: butler
1.94 MiB — CAVS `.cavsplan` 1.94 MiB (tie). With every byte shifted by an
unaligned insertion: butler 68.13 KiB — CAVS **4.21 KiB** (the
content-defined signature blocks survive the shift; fixed-block scans
resync more coarsely).

Full tables across every route: [ROUTE_BENCHMARKS.md](ROUTE_BENCHMARKS.md).

## Different models, honestly framed

butler is an excellent **pairwise patching workflow** with deep itch.io
integration: a patch is computed per old→new pair, uploaded, and
optimized server-side.

CAVS optimizes a different shape:

- **content-addressed release store** — package once per release, every
  version jump (v1→v10, v3→v10, reinstall) is served from the same
  immutable objects with zero per-pair work;
- **persistent chunk cache** — bytes a player already fetched are never
  fetched again, across versions and titles;
- **hybrid reconstruction** — the previous install is a first-class byte
  source even with an empty cache;
- **CDN/object-storage friendly** — immutable packfiles, no smart server
  required;
- optional **pairwise sidecars** ([PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md))
  for hot pairs where a dedicated patch wins.

CAVS does not claim to beat butler in every metric. It is okay for a
dedicated pairwise patch to win on bytes for one exact old→new pair —
the many-version benchmark (`cavs bench version-stream`) shows where the
store-once model wins instead: 10 versions of a 32 MiB build fit in a
**30.6 MiB** store that serves *any* jump directly, while full pairwise
coverage of 10 versions needs 45 patches plus full artifacts.
