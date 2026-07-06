# Route benchmarks (v0.7.0)

`cavs bench routes` compares **every delivery route for the same old→new
transition** in one table: full downloads, CAVS chunk/hybrid delivery,
the CAVS offline plan, butler's offline patch and optimized pairwise
proxies. Missing external tools are reported and skipped, never fatal;
every produced output is verified byte-identical before its size counts.

```bash
cavs bench routes --old ./Build_v1 --new ./Build_v2 \
  --butler-bin ./butler --include-pairwise-proxy --out results/routes
```

Related commands: `cavs bench butler-offline` (dedicated external butler
harness, raw JSON lines kept), `cavs bench pairwise-proxy`
(bsdiff/xdelta3 × zstd/brotli), `cavs bench version-stream`
(many-version storage), `cavs bench gen-dir` (synthetic directory pair).

## Measured results

Synthetic builds (`cavs bench gen`/`gen-dir`, seed 5), Apple M-series,
butler v15.27.0, xdelta3 3.2.0, system bsdiff. "Network bytes" is what a
client downloads; every route ended byte-identical.

### Directory build, typical release (125.8 → 126.9 MiB)

| Route | Network bytes | Diff time | Apply time | Peak RSS |
|---|---:|---:|---:|---:|
| full download (raw) | 126.89 MiB | — | 0 ms | — |
| full zstd-19 (bootstrap) | 62.12 MiB | 3.9 s | 14 ms | — |
| CAVS chunk / hybrid (wire) | 5.42 MiB | 301 ms | — | — |
| **CAVS offline plan (.cavsplan)** | **2.51 MiB** | **488 ms** | **262 ms** | streaming |
| butler offline/default patch | 2.52 MiB (+62 KiB sig) | 983 ms | 348 ms | 35 MiB |
| pairwise proxy: bsdiff+zstd-19 | 3.02 MiB | 25.0 s | 3.4 s | 2.3 GiB |
| pairwise proxy: xdelta3+zstd-19 | 3.01 MiB | 4.3 s | 3.7 s | 397 MiB |

### Single 128 MiB artifact, small change (~3% of blocks)

| Route | Network bytes | Diff time |
|---|---:|---:|
| full zstd-19 | 64.05 MiB | 5.2 s |
| CAVS chunk / hybrid (wire) | 6.06 MiB | 264 ms |
| **CAVS offline plan** | **1.94 MiB** | **439 ms** |
| butler offline | 1.94 MiB | 939 ms |
| bsdiff proxy | 1.96 MiB | 32.8 s |
| xdelta3 proxy | 1.94 MiB | 779 ms |

Four different tools land within 1% of each other — the changed bytes
are simply the floor. The differences are in time, memory and the
delivery model.

### Shifted artifact (4 KiB inserted at the head — every byte moves)

| Route | Network bytes |
|---|---:|
| **CAVS offline plan** | **4.21 KiB** |
| xdelta3 proxy | 4.34 KiB |
| bsdiff proxy | 4.59 KiB |
| CAVS chunk / hybrid (wire) | 10.90 KiB |
| butler offline | 68.13 KiB |

Content-defined blocks survive unaligned shifts; CAVS matches the
byte-level tools here and beats the fixed-block scan by 16×.

### Compressed single file (same build as one zstd blob, 62 MiB)

| Route | Network bytes |
|---|---:|
| xdelta3 / bsdiff proxy | ~2.5 MiB |
| CAVS offline plan | 21.92 MiB |
| butler offline | 21.92 MiB |

The block-level warning made concrete: the *same content change* costs
2.5 MiB in directory mode and ~22 MiB through a compressed blob. Publish
folders, not archives — `cavs preview` warns about this shape
([DIRECTORY_MODE.md](DIRECTORY_MODE.md)).

### Many-version stream (10 versions × 32 MiB, ~3% drift per release)

| Method | Storage | Adjacent updates | v1→v10 jump | Any-pair coverage |
|---|---:|---:|---:|---|
| CAVS packfile store | **30.60 MiB** (10 packfiles) | 13.70 MiB total (1.52 avg) | 8.95 MiB | every pair, same objects |
| bsdiff patches | 4.23 MiB (9 adjacent) + full artifacts | 4.23 MiB total | 3.60 MiB (dedicated patch) | needs 45 patches (O(N²)) or chain-apply |

Per-pair, bsdiff patches are smaller — that is expected and fine. The
store-once model wins on the operational axis: ten versions fit in less
space than one raw build, and *any* jump (v1→v10, v3→v10, reinstall) is
served from the same immutable objects with zero per-pair generation.

## Reading the results

- **CAVS wins or ties** on: apply time, peak memory (streaming apply with
  an 8 MiB read budget), shifted/insert-heavy changes, many-version
  storage, non-adjacent jumps, CDN/object-store shape, cache reuse.
- **Dedicated pairwise patches win** on: minimum bytes for one exact
  old→new pair on byte-scrambled or compressed inputs. That is what the
  optional [pairwise sidecars](PAIRWISE_SIDECARS.md) are for.
- butler offline numbers are the **offline/default patch**, not itch.io's
  backend-optimized patch — see [BUTLER_COMPARISON.md](BUTLER_COMPARISON.md).
