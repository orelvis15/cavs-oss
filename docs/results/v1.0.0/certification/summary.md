# CAVS Certification Report

Result: **PASS WITH WARNINGS**

Profile: `strict` · Mode: directory

Old build:
  dataset/Build_v1

New build:
  dataset/Build_v2

Recommended route:
  CAVS offline plan (.cavsplan)

Why:
  - 2.26 MiB network — the smallest verified payload
  - 214 ms apply
  - streaming memory (no full old copy in RAM)
  - byte-identical output

Checks:
  Integrity: PASS
  Routes: PASS
  SteamPipe-style analysis: PASS WITH WARNINGS
  Regression: SKIPPED

## Integrity

| Check | Result | Details |
|---|---|---|
| old signature export+decode | PASS | 46 entries, 87.01 KiB |
| new signature export+decode | PASS | 47 entries, 87.75 KiB |
| old signature verify | PASS | every block hash matches the source |
| plan build+decode | PASS | CAVSPLAN1, 111 ops, payload 5.51 MiB (2.26 MiB on the wire) |
| no path traversal | PASS | 0 unsafe paths in 47 entries + 2 deletions |
| apply output byte-identical | PASS | verified against new signature in 213 ms (121.38 MiB from old, 5.51 MiB fresh) |
| no-op reapply | PASS | 0 files rewritten (44 detected as no-op); output byte-identical |
| corrupt signature rejected | PASS | decoder refused a bit-flipped .cavssig |
| corrupt plan rejected | PASS | decoder refused a bit-flipped .cavsplan |
| corrupted old input fails safely | PASS | a bit flipped inside a reused old range never produced a verified output |

Detailed report: `integrity.md`

## Routes

| Check | Result | Details |
|---|---|---|
| state: cold-install | PASS | bootstrap — 62.14 MiB network, 253 ms apply (62.14 MiB (estimated) over the wire · ~32.00 MiB peak RAM · 126.89 MiB temp disk · policy balanced) |
| state: cold-cache-previous | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: warm-cache | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: exact-previous-version | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: low-ram | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: slow-hdd | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| state: limited-disk | PASS | .cavsplan — 2.26 MiB network, 253 ms apply (2.26 MiB over the wire · ~40.00 MiB peak RAM · 31.72 MiB temp disk · policy balanced) |
| measured routes verified | PASS | 8 routes measured, 2 skipped (missing tools) |
| skipped: butler offline: no --butler-bin given | SKIPPED | optional dependency not installed — skipped, never selected |
| skipped: pairwise patches serve exactly one old→new pair; storage and generation cost grow with every published pair | SKIPPED | optional dependency not installed — skipped, never selected |

Detailed report: `routes.md`

## SteamPipe-style analysis

| Check | Result | Details |
|---|---|---|
| steampipe-style analysis | PASS WITH WARNINGS | 2 findings (2 non-info); est. download 16.90 MiB |
| pack analysis | PASS | pack-analysis.md |
| disk I/O estimate | PASS | 4 routes estimated |

Detailed report: `steampipe-style.md`

## Regression

| Check | Result | Details |
|---|---|---|
| baseline | SKIPPED | no --baseline provided |

## Metrics

| Metric | Value |
|---|---:|
| apply_ms | 214 ms |
| byte_identical | 1 |
| diff_ms | 506 ms |
| full_download_bytes | 126.89 MiB |
| network_bytes | 2.26 MiB |
| plan_bytes | 2.26 MiB |
| plan_inline_bytes | 5.51 MiB |
| plan_ops_total | 111 |
| signature_new_bytes | 87.75 KiB |
| signature_old_bytes | 87.01 KiB |

---

CAVS certifies game updates locally before release. CAVS is not a CDN, marketplace, SaaS, DRM system or game store; SteamPipe-style figures are estimates from a public model, never Valve's implementation.
