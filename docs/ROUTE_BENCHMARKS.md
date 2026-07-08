# Route benchmarks (v0.7.0 / v0.8.0)

## Full-pipeline comparison (v0.8.0)

`cavs bench full-pipeline` measures every CAVS route **and the complete
butler pipeline — default `diff` and optimized `rediff --rediff-quality
9`** — on one transition. The *CAVS auto-route* row is what the
[delivery planner](DELIVERY_PLANNER.md) would actually serve (smallest
payload; near-ties broken by apply time). CAVS apply times and peak RSS
are measured by running the real `cavs` binaries under `/usr/bin/time`,
exactly like the external tools. butler v15.28.0, bsdiff 4.3, xdelta3
3.2.0, Apple M3 Pro; every output verified byte-identical.

Raw outputs, the full environment, the **exact commands to reproduce
A–H** and the **known-tradeoffs table** live in
[results/v0.8.0/](results/v0.8.0/README.md).

### A — Directory build, typical release (125.8 → 126.9 MiB)

| Route | Download | Generate | Apply | Apply RSS |
|---|---:|---:|---:|---:|
| **CAVS auto-route (plan)** | **2.51 MiB** | **539 ms** | 391 ms | **23 MiB** |
| CAVS sidecar (.cavspatch, per-file) | 2.51 MiB | 29.6 s¹ | 991 ms | 22 MiB |
| butler diff (default) | 2.52 MiB (+62 KiB sig) | 1.0 s | 331 ms | 35 MiB |
| butler rediff q9 (optimized) | 2.51 MiB | 11.6 s | 403 ms | 97 MiB |
| pairwise proxy xdelta3+zstd-19 | 3.01 MiB | 4.1 s | 3.4 s | — |
| pairwise proxy bsdiff+zstd-19 | 3.02 MiB | 25.0 s | 3.5 s | — |

Verdict vs the optimized pipeline: bytes tie, apply tie, **RAM 4.2×
lower, generation 21× faster**. ¹The sidecar's generation time is the
price of measuring every candidate per file; the planner simply doesn't
pick it here.

### B — Shifted artifact (4 KiB inserted at the head, 128 MiB)

| Route | Download | Generate | Apply | Apply RSS |
|---|---:|---:|---:|---:|
| **CAVS auto-route (plan)** | **4.21 KiB** | **324 ms** | **170 ms** | **11 MiB** |
| butler diff (default) | 68.13 KiB | 807 ms | 163 ms | 21 MiB |
| butler rediff q9 (optimized) | 11.39 KiB | 12.1 s | 370 ms | 94 MiB |
| pairwise proxy xdelta3 | 4.34 KiB | 470 ms | 364 ms | — |
| pairwise proxy bsdiff | 4.59 KiB | 24.8 s | 222 ms | — |

CAVS wins **every column** against the optimized pipeline: 37% of its
bytes, 2.2× faster apply, 12% of its RAM, 37× faster generation — and it
beats raw bsdiff/xdelta3 on bytes too.

### C — Compressed blob (same build shipped as one .tar.zst, 62 MiB)

| Route | Download | Generate | Apply | Apply RSS |
|---|---:|---:|---:|---:|
| CAVS block routes (plan/chunks) | 21.9–23.7 MiB | 0.1–2.7 s | 577 ms | 53 MiB |
| **CAVS auto-route (sidecar: xdelta3 per file)** | **2.53 MiB** | 32.5 s | 319 ms | 74 MiB |
| butler diff (default) | 21.92 MiB | 801 ms | 247 ms | 54 MiB |
| butler rediff q9 (optimized) | 2.90 MiB | 8.9 s | 294 ms | 92 MiB |
| pairwise proxy xdelta3+zstd-19 | 2.53 MiB | 669 ms | 177 ms | — |

The known weak case is closed: the strategy optimizer detects the
high-entropy blob and routes it through a byte-level delta, so CAVS no
longer pays 21.9 MiB where 2.5 MiB suffices — and lands 13% *below* the
optimized pipeline's bytes, matching the best raw tool. (The real fix is
still publishing folders — see A.)

### D — Many-version storage (10 × 32 MiB, ~3% drift)

| Model | Storage | Serves |
|---|---:|---|
| **CAVS store + 3 hot-pair sidecars** | **35.91 MiB** (30.60 + 5.31) | any jump + optimized previous/top-installed |
| all-pairs bsdiff | 144.23 MiB in 45 patches | every pair, no reinstall source |

75% less storage than all-pairs one-hop coverage; the hot pairs come from
`cavs patch-policy` (previous, top-installed shares). All-pairs is the
theoretical baseline only — the v1.1.0 patch policy benchmark
([PATCH_POLICY_BENCHMARK.md](PATCH_POLICY_BENCHMARK.md)) also measures the
practical adjacent/ladder/base/hot-pair policies real systems deploy.

### E — Interrupted apply & mod preservation

`cavs test apply-recovery` on the directory pair: 10 SIGKILLed applies
at ramping delays all recovered by re-running (journal resume); no torn
files ever observed; user mod files survived every run; a corrupt plan
is rejected with the install untouched; a corrupted old install either
fails cleanly or **self-heals** (deduplicated content provides the
damaged range from another file) with the output verified
byte-identical.

### Developer workflow (126 MiB directory build)

| Step | Time |
|---|---:|
| `cavs signature export` | 0.28 s |
| `cavs preview` (+renames, +blob detection) | 0.35 s |
| `cavs diff-plan` (portable plan) | 0.42 s |
| `cavs verify-install` | 0.10 s |

---

# v0.7.0 route benchmarks

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
| bsdiff patches | 4.23 MiB (9 adjacent) + full artifacts | 4.23 MiB total | 3.60 MiB (dedicated patch) | all-pairs one-hop needs 45 patches; practical policies chain or budget instead |

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
- butler offline numbers here are the **default patch**; the optimized
  patch (`rediff q9`) is measured directly in the v0.8.0 section above —
  see [BUTLER_COMPARISON.md](BUTLER_COMPARISON.md).
