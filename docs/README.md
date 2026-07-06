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
- [PAPER.md](PAPER.md) — the technical paper: design, rationale, and results.
