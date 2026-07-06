# Optimized pairwise sidecars (`.cavspatch`, experimental v0.7.0)

For a *hot* old→new pair — "previous release → latest" for most players —
a dedicated byte-level patch can beat chunked delivery on wire bytes.
Sidecars make that an **optional route inside CAVS** without changing the
architecture: content stays content-addressed; a sidecar is just a
cheaper edge for one specific version jump.

```bash
# Generate (needs bsdiff or xdelta3 on PATH):
cavs optimize-patch --old game_v1.pck --new game_v2.pck \
  --algo xdelta3 --compression zstd-19 --out patches/v1_to_v2.cavspatch

# Apply (verifies both ends, atomic rename):
cavs apply-patch --old game_v1.pck --patch patches/v1_to_v2.cavspatch -o game_v2.pck

# Inspect:
cavs file patches/v1_to_v2.cavspatch
```

## Format

`CAVSPCH1` magic; algo + compression labels; size and full BLAKE3 of
*both* the old and new artifacts; the compressed patch payload; a BLAKE3
integrity trailer over the whole file. Apply refuses the wrong old
version (`CAVS-E-APPLY-HASH-MISMATCH`), verifies the produced output
before the atomic rename, and a corrupt sidecar fails at decode
(`CAVS-E-PLAN-CORRUPT`).

## The O(N²) warning

A sidecar serves **exactly one pair**. With N published versions there
are N·(N−1)/2 pairs — at 10 versions that is already 45 patches, each of
which must be generated, stored and invalidated correctly. Measured in
`cavs bench version-stream`: 10 versions of a 32 MiB build fit in a
30.6 MiB CAVS store that serves any jump; full pairwise coverage would
need 45 dedicated patches plus full artifacts for reinstalls.

Recommended policy — generate sidecars only for configured hot pairs:

```text
pairs = previous → latest        (most players)
        latest-stable → latest   (slow channel)
max_pairs_per_release = 3
```

Route selection stays cost-based: use the sidecar when the client has the
exact old version and the sidecar is smaller than the hybrid route;
otherwise fall back to chunk/hybrid/bootstrap delivery. In v0.7.0 the
comparison is offline (`cavs bench routes` includes both); server-side
advertising of sidecars is future work.

## Measured (128 MiB artifact, small change)

| Route | Wire bytes | Gen time | Peak RSS |
|---|---:|---:|---:|
| sidecar xdelta3+zstd-19 | 1.94 MiB | 1.0 s | 397 MiB |
| sidecar bsdiff+zstd-19 | 1.96 MiB | 33 s | 2.3 GiB |
| CAVS offline plan | 1.94 MiB | 0.4 s | streaming |
| CAVS chunk/hybrid wire | 6.06 MiB | 0.3 s | streaming |

On this workload the sidecar ties the CAVS plan — sidecars earn their
keep on byte-scrambled or compressed single-file inputs where block
reuse collapses (see the compressed-blob row in
[ROUTE_BENCHMARKS.md](ROUTE_BENCHMARKS.md), where xdelta3 wins 2.5 MiB
vs 22 MiB).

`cavs bench pairwise-proxy` measures this class against butler-style
optimized patches; results are always labeled **proxy**, never as
official itch.io backend numbers ([BUTLER_COMPARISON.md](BUTLER_COMPARISON.md)).
