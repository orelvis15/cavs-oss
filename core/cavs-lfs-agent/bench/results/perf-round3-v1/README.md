# perf-round3-v1 — Round 3 (metadata & many-files) WAN validation

Frozen results of the Round 3 A/B benchmark: session meta-packs +
batching metadata resolver + chunk-map v2 runs + AIMD adaptive
concurrency, against the Round 2.1 agent.

## Setup

- **Harness**: `bench/http-bench.sh` (cold `git clone` through the LFS
  agent from an HTTP static remote; `bench/range_server.py` serves the
  export tree with real Range/206 support and `LATENCY_MS` of sleep per
  response to emulate CDN RTT).
- **Agent A** (baseline): `cavs-lfs-agent` built from `main` @ `f8def72`
  (v1.6.0, Round 2.1).
- **Agent B**: `cavs-lfs-agent` built from
  `feature/round3-metadata-and-scale` @ `88b1fa0` (adaptive concurrency
  default, `--connections 0`).
- **Push agent**: Round 3 (the origin tree carries `meta/` session
  meta-packs; per-asset `manifest.json`/`chunk-map.json` stay v1, which
  is what Agent A reads — both agents clone the same origin).
- 3 runs per network profile, fresh origin + fresh cache per run; every
  clone is sha256-verified against the source dataset (the harness
  aborts on any mismatch). Machine: macOS Apple Silicon / APFS, no
  concurrent load. Release builds.

## Results (median of 3)

### WAN 25 ms (`LATENCY_MS=25`)

| scenario | metric | round 2.1 | round 3 | Δ |
|---|---|---:|---:|---:|
| big-binary | time_s | 3.44 | 3.28 | −4.7% |
| big-binary | http_requests | 47 | 47 | = |
| many-files | time_s | 28.52 | **12.76** | **−55.3%** |
| many-files | http_requests | 854 | **359** | **−58.0%** |

### localhost 0 ms (`LATENCY_MS=0`)

| scenario | metric | round 2.1 | round 3 | Δ |
|---|---|---:|---:|---:|
| big-binary | time_s | 3.61 | 3.47 | −3.9% |
| big-binary | http_requests | 47 | 47 | = |
| many-files | time_s | 4.99 | 4.37 | −12.4% |
| many-files | http_requests | 854 | 359 | −58.0% |

### Where the requests went (agent session breakdown, many-files)

Round 2.1 issued 2 metadata GETs per object (250 × manifest.json +
chunk-map.json = 500 of the 854 requests). Round 3's resolver issued
**5 metadata requests total** for the same clone:

```text
session: 250 downloads | metadata 5 req / 82 ms
         (l1 246 · l2 0 · packs 4 · prefetched 321 · fallbacks 0)
         | payload 354 req | 90.8 MB wire / 90.7 MB useful
```

1 × `meta/index.json` + 4 × session meta-packs (one per pushed version);
every other object resolved from the in-process L1 cache filled by pack
prefetch. Payload requests are unchanged (354) — the win is purely the
metadata path, which is why the WAN (round-trip-priced) improvement is
~4.4× while localhost improves modestly.

## Round 3 acceptance gates (spec §4.10 / §2.2)

| Gate | Target | Measured | Verdict |
|---|---|---|---|
| Metadata requests, many-files | −70% | 500 → 5 (−99%) | ✅ |
| WAN time, many-files vs round 2.1 | −35% | −55.3% | ✅ |
| Total requests, many-files | < 400 | 359 | ✅ |
| big-binary regression | < 5% | −4.7% (improved) | ✅ |
| Cold clone with caches enabled | no worse than −5% | −12.4% (improved) | ✅ |
| Read amplification | p95 ≤ 1.25× | 90.8/90.7 = 1.002× | ✅ |
| SHA-256 integrity, all clones | 100% | 100% (12/12 clones verified) | ✅ |

Index scale gates (measured separately, `cargo test -p cavs-store
--release -- --ignored index_scale_segmented`): 1 M-chunk segmented
index — create + warm open + 1000 random lookups in ~1.6 s total, warm
open sub-second (asserted). The chunk table is mmapped and never loads
into RAM; corruption is detected per segment.

## Raw data

- `http-bench-wan25.csv` — 3 runs, WAN 25 ms
- `http-bench-local.csv` — 3 runs, localhost

## Reproduce

```sh
cargo build --release -p cavs-lfs-agent            # round 3 agent
git worktree add /tmp/wt-main f8def72 && (cd /tmp/wt-main && cargo build --release -p cavs-lfs-agent)
cd core/cavs-lfs-agent
LATENCY_MS=25 AGENT_A=/tmp/wt-main/target/release/cavs-lfs-agent \
  AGENT_B=../../target/release/cavs-lfs-agent \
  AGENT_PUSH=../../target/release/cavs-lfs-agent \
  bash bench/http-bench.sh big-binary many-files
```

## Notes / caveats

- The WAN emulation prices every request at a fixed 25 ms server-side
  delay; it does not model bandwidth caps or loss (spec §11.5's fuller
  profile matrix remains future work).
- A direct Xet comparison (spec §12) requires the same dataset pushed
  through hf-xet on the same machine/storage; not part of this freeze.
- Localhost numbers have ~±0.5 s of scheduling noise; request counts are
  exact and load-independent.
