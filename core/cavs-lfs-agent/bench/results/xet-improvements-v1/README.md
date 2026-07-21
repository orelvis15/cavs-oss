# xet-improvements-v1 — session finalize + BG4 (2026-07-21)

Validation run for the two Xet-inspired changes on
`feature/xet-inspired-improvements` (see `deep-research-report.md` and the
CHANGELOG "Unreleased" entry):

1. **Session-batched publish**: packs aggregate across a whole push and the
   store ledger/export are committed once at terminate, instead of one pack
   + one `index.json` rewrite per object.
2. **BG4 chunk codec**: per-chunk zstd vs byte-grouping-4 + zstd, attempted
   only when plain zstd underperforms (numeric payloads).

Method: `bench/run.sh` (with the new `tensor` scenario) run twice on the
same machine, same deterministic datasets — `results-branch.csv` with the
branch agent (`SYSTEMS="lfs cavs"`), `results-main.csv` with the agent
built from `main` at 141c71a (`SYSTEMS="cavs"`). All sha256 verification
gates passed in both runs.

## Branch agent vs main agent

| Scenario | Metric | main | branch | Δ |
|---|---|---:|---:|---:|
| tensor (32 MiB f32 weights, 3v) | stored pack data | 27.94 MiB | 23.31 MiB | **−16.6%** |
| tensor | remote store total | 63,185 KiB | 53,694 KiB | **−15.0%** |
| many-files (250 files, 4v) | push total | 20.1 s | 17.4 s | **−13.6%** |
| big-binary (100→104 MiB, 5v) | push total | 19.5 s | 18.9 s | −2.8% |
| full-rewrite (48 MiB, 2v) | push total | 9.6 s | 9.0 s | −5.8% |
| compressible (64 MiB, 4v) | push total | 13.8 s | 14.8 s | +7.3% (†) |

(†) Re-run of compressible alone: 13.91 s (main) vs 14.10 s (branch) —
within noise; stored bytes are byte-identical, so no BG4 chunk won there
and the codec added no storage cost.

Storage, update-download and clone bytes are **byte-identical** between
agents on every non-tensor scenario (content-addressed packs coincide):
the batching changed *when* things are published, not *what*; BG4 only
fires where it pays.

## Branch agent vs vanilla Git LFS (same run)

| Scenario | Metric | LFS | CAVS branch | Δ |
|---|---|---:|---:|---:|
| tensor | remote store | 98,330 KiB | 53,694 KiB | −45% |
| tensor | update download | 98,304 KiB | 38,111 KiB | −61% |
| tensor | push total | 14.9 s | 11.2 s | −25% |
| many-files | push total | 18.5 s | 17.4 s | −6% |

The many-files aggregation effect is also structural: a 250-object push now
produces one ~128 MiB-bounded pack per rollover instead of 250 packs
(asserted by `batch_uploads_share_one_session`), and `index.json` is
written once per push instead of 250 times.
