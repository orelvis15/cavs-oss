# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2]

The v2 efficiency release: cold installs now cost *less* than downloading the
full compressed release, while updates keep their savings. Measured on real
games (tps-demo, GDQuest, Marble, Godot 4.7 exports): cold install −7% to
−13% vs the zstd-3 full download (previously +2–4% overhead), updates
−81.2% to −99.3%, warm re-fetch 0 bytes, byte-identical reconstruction.

### Added

- **Dual delivery route.** `cavs pack --bootstrap` emits a
  `<output>.bootstrap.zst` sidecar (whole artifact, zstd-19) recorded in the
  container metadata. `cavs-server` verifies it at load, estimates the
  chunk-path payload per session and routes cold clients (<5% cached) to the
  bootstrap when it is ≥2% cheaper; new `GET /api/assets/{asset}/bootstrap`
  endpoint (immutable, streamed) and bootstrap metrics. `cavs-client`
  downloads it streaming, verifies BLAKE3 + SHA-256, installs atomically and
  **seeds its chunk cache** from the manifest chunk plan, so the next update
  is incremental. Chunk path remains the fallback everywhere.
- **Payload classifier.** Format magic + extension + sampled entropy + zstd
  probe decide the candidate chunk profiles: precompressed content gets large
  fixed chunks, engine packs get CDC, text gets small CDC.
- **Chunk-profile auto-sweep.** Six candidate profiles (fixed 256K/512K/1M,
  FastCDC 64K/128K/256K) measured on the real bytes and scored by a weighted
  cost model. `cavs pack --profile auto` applies the cheapest; the new
  `cavs sweep <build> [--prev <old>]` prints the full table (and `--json`).
  `--prev` accepts the previously *published* `.cavs` so reuse is measured
  against real chunk hashes and profile choice stays consistent across a
  version stream.
- Fetch stats now report `delivery_mode`, `seeded_chunks` and `seed_ms`.

### Changed

- Session-open responses now include the delivery-route decision (additive,
  optional fields — fully backward compatible with 0.1.x clients/servers).
- Godot plugin version aligned to 0.1.2 (no behavioural change; the GDScript
  client keeps using the chunk path).

## [0.1.1]

### Fixed

- CI: build all workspace binaries before running tests, so the `cavs-client`
  integration tests can find the `cavs-server` / `cavs` binaries they spawn on a
  clean runner.

### Changed

- Release workflow now publishes all crates to crates.io automatically on a
  version tag (in dependency order), so releases no longer require a manual
  `cargo publish`.

## [0.1.0]

Initial public release.

### Added

- `.cavs` content-addressable container format (FastCDC + zstd + BLAKE3),
  with a global Merkle root, per-file SHA-256, and optional Ed25519 signatures.
- `cavs` CLI: pack / unpack / info / verify / keygen, and a global
  content-addressable store (`store add` / `rm` / `gc` / `stat`) with reference
  counting and garbage collection.
- `cavs-server`: stateful HTTP/HTTPS origin with per-session have-set planning
  (exact or Bloom filter), CVSP binary batches, `--store` mode, HLS passthrough,
  and Prometheus metrics.
- `cavs-client`: native streaming client with a persistent cache and atomic,
  verified reconstruction (resumable, retry-safe).
- Godot 4 plugin: pure-GDScript runtime client that mounts reconstructed packs
  with `load_resource_pack()`.
- `cavs-steam`: SteamPipe update-size analyzer for game builds.
- Documentation: format specification, architecture, benchmarks, and paper.

[Unreleased]: https://github.com/orelvis15/cavs-oss/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/orelvis15/cavs-oss/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/orelvis15/cavs-oss/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/orelvis15/cavs-oss/releases/tag/v0.1.0
