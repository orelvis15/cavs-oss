# perf-round2.1-v1 — hardening validation: WAN latency + amplification guard (2026-07-21)

Validation run for round 2.1 ("hardening antes de merge") on
`feature/xet-inspired-improvements` (commit 6fa1aeb). Raw CSV in
`http-bench.csv`. Same machine for everything; request counts are
deterministic.

## WAN cold clone (LATENCY_MS=25 → ~25 ms/request, the CDN case)

`bench/http-bench.sh` with the new `LATENCY_MS` knob in
`range_server.py`. agent-a = r1 (897b8cf, per-chunk GETs, pre-coalescing);
agent-b = r2.1 (current).

| Scenario | Metric | r1 | r2.1 | Δ |
|---|---|---:|---:|---:|
| big-binary (104 MiB) | HTTP requests | 6,179 | **47** | −99.2% |
| big-binary | cold clone time | 26.78 s | **3.66 s** | **−86%** |
| many-files (250 files) | HTTP requests | 5,741 | **854** | −85% |
| many-files | cold clone time | 46.46 s | **29.29 s** | **−37%** |

This is the number the localhost round-2 run could only hint at: with a
realistic per-request cost, the request collapse converts directly into
wall time. (many-files' remaining 29 s is dominated by the 500 per-object
metadata GETs the LFS protocol forces — 2 per object, serialized by
git-lfs — the round-3 metadata-cache work targets exactly that.)

Gate «mejora WAN > 30% frente a pre-coalescing»: **passes** in both
scenarios (−86% / −37%).

## Amplification guard cost (LATENCY_MS=0, r2 = 7c106a7 vs r2.1)

The 15%-waste cap can only *split* groups, so its cost is extra requests:

| Scenario | r2 requests | r2.1 requests | Δ |
|---|---:|---:|---:|
| big-binary | 46 | 47 | +1 |
| many-files | 840 | 854 | +1.7% |

Times were within run-to-run noise (±0.4 s). In exchange, per-group read
amplification is now bounded at 1.15× **by construction** (previously a
sparse update could coalesce 64 KiB gaps around tiny chunks and download
~65× the useful bytes in pathological patterns). Gate «amplificación p95
< 1.25×»: holds structurally; `FetchStats.useful_bytes` exposes it.

Gate «reducción de requests ≥ 80% en many-files»: 5,741 → 854 = −85%,
still passes with the guard on.

## What round 2.1 added (no benchmark, correctness-gated by tests)

- Crash-safe `index.bin` writes (staged + fsync + read-back verify +
  atomic rename), `index.bin.prev` fallback at open, formal versioned
  header, allocation guards. Tests: corruption/truncation/both-corrupt,
  stale tmp, future-version rejection.
- GC quarantine: two-stage deletion with restore-on-reference (sweep and
  open). Tests: live-pack restore, staged orphan aging.
- Selective retries: transport/short-read transparent retry; per-chunk
  re-request on hash mismatch inside a coalesced range, with stable
  `CAVS-E-*` codes. Tests: mock flaky source (heals / persists / truncates).
- Global inflight-byte backpressure (`CAVS_FETCH_MAX_INFLIGHT_BYTES`,
  default 128 MiB). Test: weighted-semaphore blocking + oversize clamp.

sha256 verification gates passed in every clone of every run.
