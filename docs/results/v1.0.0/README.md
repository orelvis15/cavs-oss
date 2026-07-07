# v1.0.0 certification benchmark results — raw outputs & reproduction

Raw outputs of the v1.0.0 release certification suite: the first release
where the whole benchmark matrix runs through one command family,
`cavs certify`. Regenerate everything with
[`scripts/run-all.sh`](scripts/run-all.sh); every dataset is
deterministic (seeds below), so the byte counts reproduce exactly and
the timings reproduce within normal wall-clock jitter.

> Every SteamPipe-style figure is an estimate from a public fixed-1MiB
> model — not Valve's exact SteamPipe implementation
> (see [STEAMPIPE_STYLE_MODEL.md](../../STEAMPIPE_STYLE_MODEL.md)).

## Environment

| | |
|---|---|
| OS | macOS 26.5.1 (Darwin 25.5.0), arm64 |
| CPU | Apple M3 Pro (12 cores: 6P + 6E) |
| RAM | 36 GiB |
| Disk | internal NVMe SSD, APFS |
| rustc | 1.96.1 (release build, `lto = "thin"`, `codegen-units = 1`) |
| cavs | 1.0.0 |
| butler | not installed for this run (routes report it as skipped) |
| Dataset seeds | gen-dir 7 · steampipe-cases 9 |
| Date | 2026-07-07 |

Each certification directory additionally embeds its own
`environment.json`, `dependencies.json` and `commands.sh` — a result is
never anonymous, and `certification/repro.tar.zst` is the deterministic
reproducibility bundle exported by the run itself.

## What is here

| Directory | Plan benchmark | Content |
|---|---|---|
| `certification/` | A (integrity), B (route matrix), H (disk I/O), J (repro bundle) | `cavs certify --profile strict` on the 128 MiB gen-dir pair: integrity + corruption smoke, per-client-state route decisions, measured route matrix, SteamPipe-style analysis, pack analysis, I/O estimates, repro bundle |
| `certification-ci/` | C (regression guard) | The same pair re-certified with `--profile ci` against `baseline.json` |
| `baseline.json` | C | Regression baseline recorded by the strict run (`--save-baseline`) |
| `godot/` | D (Godot PCK) | `cavs certify godot` on the deterministic `godot-pck-localized` case: byte-identical PCK on every route, analyzer report, plugin API surface |
| `workspace/` | I (workspace/depot/install-plan) | `cavs certify workspace` over a 5-depot app (base, windows, linux, lang-es, dlc1), two builds, promote/rollback previews, install plans |
| `steampipe-cases/` | E (SteamPipe-style analyzer) | The 12-case pack-pathology matrix (seed 9), also the source of the Godot dataset |

## Headline numbers

From `certification/summary.json` (125.83 MiB directory build, ~2%
drift between versions, seed 7):

| Metric | Value |
|---|---:|
| Full download | 126.89 MiB |
| CAVS `.cavsplan` network | 2.26 MiB (−98.2%) |
| Plan apply | 214 ms, byte-identical, verified |
| Signatures (old/new) | 87.0 KiB / 87.8 KiB |
| No-op reapply | 0 files rewritten (44 no-op) |
| Corruption smoke (sig/plan/old) | all rejected cleanly |
| Regression vs own baseline | PASS (byte counts exact) |

From `godot/godot.json` (3 MiB PCK, one edited resource):

| Route | Result |
|---|---|
| `.cavsplan` | byte-identical |
| chunk / hybrid (wire) | 72.63 KiB, byte-identical |
| bootstrap (full zstd-19) | 2.50 MiB, byte-identical |
| plugin API (`fetch`/`fetch_async`/`ensure_pack`) | unchanged |

Overall verdicts: strict and ci certifications **PASS WITH WARNINGS**
(exit 2 — the synthetic dataset deliberately contains pack-layout
pathologies the analyzer must flag; integrity, routes and regression
sections all PASS); Godot and workspace certifications **PASS** (exit 0).
