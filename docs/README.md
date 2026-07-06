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
- [PAIRWISE_SIDECARS.md](PAIRWISE_SIDECARS.md) — v0.7.0 experimental:
  optional `.cavspatch` sidecars for hot version pairs, and the O(N²) risk.
- [PAPER.md](PAPER.md) — the technical paper: design, rationale, and results.
