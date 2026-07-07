# Documentation

- [FORMAT.md](FORMAT.md) — the `.cavs` binary format, byte for byte, plus the
  global store layout and the content-signature scheme.
- [ARCHITECTURE.md](ARCHITECTURE.md) — how the system fits together: crates,
  the update flow, the integrity chain, storage vs egress dedup.
- [BENCHMARKS.md](BENCHMARKS.md) — measured results on real games, comparisons
  vs xdelta/bsdiff/rdiff/rsync, parameter sweeps, client cost, and the honest
  negatives.
- [HYBRID_RECONSTRUCTION.md](HYBRID_RECONSTRUCTION.md) — v0.6.0: reusing the
  previously installed version as a byte source, no-op detection, directory
  mode, and the reconstruction plan model.
- [SIGNATURE_FORMAT.md](SIGNATURE_FORMAT.md) — v0.6.0: the compact `.cavssig`
  old-version signature format and its weak/strong hashing.
- [DELTA_COMPARISON.md](DELTA_COMPARISON.md) — v0.6.0: CAVS measured against
  block-based and byte-level delta patchers, with honest framing.
- [OFFLINE_TOOLKIT.md](OFFLINE_TOOLKIT.md) — v0.7.0: sign, preview, diff,
  apply and verify builds locally, with no CAVS server involved.
- [CAVSPLAN_FORMAT.md](CAVSPLAN_FORMAT.md) — v0.7.0: the `.cavsplan` offline
  reconstruction plan format, byte for byte.
- [DIRECTORY_MODE.md](DIRECTORY_MODE.md) — v0.7.0: stable directory/container
  packaging — ignore rules, path safety, staged mod-friendly applies.
- [ROUTE_BENCHMARKS.md](ROUTE_BENCHMARKS.md) — v0.7.0: every delivery route
  for the same transition, measured — full downloads, CAVS routes, butler
  offline, pairwise proxies, many-version storage.
- [BUTLER_COMPARISON.md](BUTLER_COMPARISON.md) — v0.7.0: the external butler
  benchmark harness and how its results are (and are not) comparable.
- [PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md) — v0.8.0: `.cavspatch` v2
  optimized sidecars with per-file strategy selection, memory budgets,
  the hot-pair policy and the O(N²) rule.
- [DELIVERY_PLANNER.md](DELIVERY_PLANNER.md) — v0.8.0: the route planner —
  no-op/chunks/hybrid/plan/sidecar/bootstrap scored under client profiles.
- [STEAMPIPE_STYLE_MODEL.md](STEAMPIPE_STYLE_MODEL.md) — v0.9.0: the public
  fixed-1MiB update model — exactly what it estimates and what it does not
  claim.
- [STEAMPIPE_COMPARISON.md](STEAMPIPE_COMPARISON.md) — v0.9.0: the model and
  every CAVS route measured across pack pathologies, depots, I/O and
  many-version streams.
- [WHY_NO_STEAM_ANALYZER_PRODUCT.md](WHY_NO_STEAM_ANALYZER_PRODUCT.md) —
  v0.9.0: why SteamPipe-style analysis is a CAVS command family, not a
  separate product.
- [BUILD_UPDATE_ANALYZER.md](BUILD_UPDATE_ANALYZER.md) — v0.9.0: `bench
  steampipe-style`, `analyze steampipe` and `publish-preview` — numbers,
  diagnosis, decision.
- [PACK_FILE_OPTIMIZATION.md](PACK_FILE_OPTIMIZATION.md) — v0.9.0: pack-file
  failure modes (churn, shuffling, TOC, compression) and the layout rules
  that fix them, measured.
- [DEPOTS_BRANCHES_WORKSPACE.md](DEPOTS_BRANCHES_WORKSPACE.md) — v0.9.0: the
  local app/depot/branch/build workspace, sharing analysis and install-plan
  simulation.
- [ROUTE_PLANNER.md](ROUTE_PLANNER.md) — v0.9.0: `plan-update` — routes
  scored under explicit policies (network/CPU/RAM/disk) per client state.
- [IO_ESTIMATOR.md](IO_ESTIMATOR.md) — v0.9.0: local disk I/O per route with
  configurable device profiles; why small downloads can still be slow.
- [LOCAL_CONTENT_SERVER.md](LOCAL_CONTENT_SERVER.md) — v0.9.0: `cavs serve` —
  the development-only workspace server (branches, depots, chunks, ranges).
- [GODOT_PCK_ANALYZER.md](GODOT_PCK_ANALYZER.md) — v0.9.0: PCK-aware
  analysis that maps changed bytes back to `res://` paths.
- [CERTIFICATION.md](CERTIFICATION.md) — v1.0.0: `cavs certify` — the
  release-readiness suite: integrity, routes, regressions, Godot,
  workspace, profiles, exit codes and report schemas.
- [TRY_CAVS.md](TRY_CAVS.md) — v1.0.0: try CAVS on your own game build in
  minutes — install, preview, certify, use cases, troubleshooting, FAQ.
- [ROUTE_SELECTION.md](ROUTE_SELECTION.md) — v1.0.0: the certified route
  selection rules across client states, and how decisions are scored and
  explained.
- [CI.md](CI.md) — v1.0.0: certification in GitHub Actions / GitLab CI —
  the ci profile, JSON output, exit-code gates and baselines.
- [REPRODUCIBILITY.md](REPRODUCIBILITY.md) — v1.0.0: deterministic
  certification bundles others can verify — contents and guarantees.
- [COMPATIBILITY.md](COMPATIBILITY.md) — v1.0.0: what "stable" means for
  v1.x — frozen CLI families, formats, schemas, exit codes and the
  deprecation rules.
- [FILE_FORMATS.md](FILE_FORMATS.md) — v1.0.0: index of every on-disk
  format and JSON schema with its stability status.
- [GODOT_PLUGIN.md](GODOT_PLUGIN.md) — v1.0.0: the stable Godot runtime
  API (`fetch`/`fetch_async`/`ensure_pack`) and PCK certification.
- [RELEASE_CHECKLIST.md](RELEASE_CHECKLIST.md) — v1.0.0: the gate every
  release runs before tagging.
- [PAPER.md](PAPER.md) — the technical paper: design, rationale, and results.
