# perf-round2-v1 — coalesced range fetch, binary ledger, orphan GC (2026-07-21)

Validation run for round 2 on `feature/xet-inspired-improvements`
(commit 7a8382c), vs the same branch one commit earlier (897b8cf = round 1,
"r1"). Everything measured on the same machine.

## Headline: cold clone over HTTP (the CDN scenario)

`bench/http-bench.sh` — datasets pushed to a directory remote, the export
tree served over localhost HTTP **with Range support**
(`bench/range_server.py`), cloned cold once per agent. Raw CSV in
`http-cold-clone.csv` (`prev` = r1, `new` = r2).

| Scenario | Metric | r1 (per-chunk GETs) | r2 (coalesced) | Δ |
|---|---|---:|---:|---:|
| big-binary (104 MiB) | HTTP requests | 6,179 | **46** | **−99.3%** |
| big-binary | cold clone time | 4.51 s | 3.50 s | −22% |
| many-files (250 files) | HTTP requests | 5,741 | **840** | **−85%** |
| many-files | cold clone time | 6.60 s | 4.91 s | −26% |

Request counts are deterministic. The times above are on localhost
(~0.2 ms/request); on a real CDN at 20–50 ms RTT with 8 connections, 6k
sequential-ish round-trips are tens of seconds — the request collapse is
the structural win, and it also collapses per-request CDN/S3 billing.
(many-files' 840 = 250 manifests + 250 chunk-maps + ~340 coalesced ranges;
metadata requests are per-LFS-object by protocol.)

## Store ledger: binary index.bin vs pretty JSON

Scale probe `index_scale_1m_chunks_bin_vs_json` (release, 1M chunks):

| | index.bin | index.json | Δ |
|---|---:|---:|---:|
| size | 72.0 MB | 309.1 MB | **−77%** |
| decode | 204 ms | 393 ms | −48% |
| encode | 250 ms | 236 ms | ~par |

Plus a BLAKE3 seal (corruption now detected at open, not at first bad
read). Legacy `index.json` stores are read and migrated on the next save.

## Directory-remote harness (interleaved A/B, `results-ab-*.csv`)

Byte counters: remote store shrinks −0.8…−1.4% (the ledger file itself);
update/clone downloaded bytes are byte-identical. Wall times on a local
directory remote have ±20% run-to-run noise at this dataset scale and are
**not** treated as evidence in either direction (an earlier non-interleaved
run suggested −35% pushes across the board; re-measuring r1 back-to-back
showed that was background machine load, not the code). All sha256
verification gates passed in every run.
