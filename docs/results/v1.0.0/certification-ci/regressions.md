# Regression Report

Result: **PASS**

| Metric | Baseline | Current | Change | Threshold | Status |
|---|---:|---:|---:|---:|---|
| apply_ms | 214 ms | 251 ms | +17.3% | 10% | PASS |
| diff_ms | 506 ms | 566 ms | +11.9% | 10% | PASS |
| full_download_bytes | 126.89 MiB | 126.89 MiB | +0.0% | 5% | PASS |
| network_bytes | 2.26 MiB | 2.26 MiB | +0.0% | 5% | PASS |
| plan_bytes | 2.26 MiB | 2.26 MiB | +0.0% | 5% | PASS |
| plan_inline_bytes | 5.51 MiB | 5.51 MiB | +0.0% | 5% | PASS |
| plan_ops_total | 111 | 111 | +0.0% | 5% | PASS |
| signature_new_bytes | 87.75 KiB | 87.75 KiB | +0.0% | 5% | PASS |
| signature_old_bytes | 87.01 KiB | 87.01 KiB | +0.0% | 5% | PASS |

| Check | Result | Details |
|---|---|---|
| byte-identical status | PASS | current: true |
| comparable metrics | PASS | 9 metrics compared |

Byte counts are exact and compared strictly against their threshold. Timing (`*_ms`) and RAM metrics additionally need an absolute delta (>250 ms / >32 MiB) before failing: single-run wall-clock jitter is not a regression.
