# Release checklist (v1.x)

The gate every CAVS release runs before tagging. v1.0.0 status is
recorded inline; future releases copy this list into the release issue.

## Certification suite

- [x] `cavs certify` exists and orchestrates all sections.
- [x] `cavs certify integrity` passes artifact / directory / PCK cases
      (byte-identical mandatory, corruption smoke in strict).
- [x] `cavs certify routes` tests the default client-state matrix and
      verifies every measured route.
- [x] `cavs certify regressions` compares against a baseline with
      configurable thresholds and per-metric exceptions.
- [x] `cavs certify godot` validates the PCK flow and the plugin API
      surface.
- [x] `cavs certify workspace` validates depots/branches/builds,
      promote/rollback previews and install plans.
- [x] `cavs certify export-repro` creates a deterministic bundle with
      no private inputs by default.

## Contracts

- [x] JSON schemas documented ([CERTIFICATION.md](CERTIFICATION.md),
      [FILE_FORMATS.md](FILE_FORMATS.md)).
- [x] Exit codes documented and frozen (0–5).
- [x] CI usage documented ([CI.md](CI.md)).
- [x] Compatibility policy published ([COMPATIBILITY.md](COMPATIBILITY.md)).
- [x] CLI `--help` matches the documentation.

## Benchmarks

- [x] Full benchmark matrix updated (`docs/results/v1.0.0/`,
      regenerable with `scripts/run-all.sh`).
- [x] Regression baseline recorded (`docs/results/v1.0.0/baseline.json`).

## Public docs

- [x] README quick start current.
- [x] Try CAVS guide published (docs/TRY_CAVS.md + landing `/try`).
- [x] Certification guide published.
- [x] Godot plugin guide published ([GODOT_PLUGIN.md](GODOT_PLUGIN.md)).
- [x] Landing page updated (version tag, Try CAVS page linked).
- [x] CHANGELOG updated.

## Positioning guardrails

- [x] No CDN / marketplace / SaaS / store claims added.
- [x] No SteamPipe exact-implementation claims (estimates are labeled).
- [x] No official itch.io/butler integration claims.
- [x] No DRM claims (signing/encryption framed as integrity tooling).
- [x] No breaking change to the Godot plugin API.

## Tagging

- [ ] Workspace version bumped and `cavs --version` agrees.
- [ ] Full test suite green (`cargo test --workspace`).
- [ ] Tag `vX.Y.Z`, create the GitHub release with
      `docs/results/<version>` linked, attach binaries when published.
