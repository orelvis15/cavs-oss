# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0] — Auto-route optimized delivery

CAVS v0.8.0 introduces auto-route optimized delivery: a planner that can
choose between chunks, hybrid reconstruction, offline plans, optimized
sidecars, bootstrap, full download, or no-op based on client state,
memory budget, and measured route cost.

In the v0.8.0 benchmark suite, CAVS auto-route matched or beat the
optimized baseline in network bytes, apply time, peak RAM, correctness,
multi-version storage, and arbitrary version jumps, while keeping
sidecar generation limited to selected hot pairs instead of requiring
all-pairs patches.

### Results

Measured on the reproducible v0.8.0 suite (synthetic builds, seed 5;
butler v15.28.0 default `diff` *and* optimized `rediff --rediff-quality
9` as baselines; bsdiff 4.3, xdelta3 3.2.0; Apple M3 Pro). Raw outputs,
environment, exact reproduction commands and known tradeoffs:
[docs/results/v0.8.0/](docs/results/v0.8.0/README.md).

- Matched optimized-baseline bytes on typical directory updates
  (2.51 MiB vs 2.51 MiB, apply within 5%) while generating 21× faster
  and using 4.2× less peak RAM (23 vs 97 MiB).
- Beat the optimized baseline on shifted artifacts: 4.21 KiB vs
  11.39 KiB, with faster apply and lower RAM.
- Closed the compressed-blob weak case with per-file xdelta3 routing:
  2.53 MiB where block routes paid 21.9 MiB on the same change.
- Reduced 10-version stream storage by 75% versus all-pairs patches
  (35.91 MiB store + hot pairs vs 144.23 MiB in 45 patches), still
  serving arbitrary version jumps.
- Enforced memory budgets at apply time: a 517 MiB-RSS bsdiff sidecar is
  refused under `--memory-budget 128MiB` and the planner serves the
  streaming plan route (27 MiB RSS) at comparable bytes.
- Preserved byte-identical output across every route, including 10
  SIGKILL-interrupted applies recovered via the journal.

Not claimed: the default `butler diff` remains faster to generate than a
full sidecar, its apply was marginally faster on the directory case, and
dedicated pairwise patches can still win bytes on inputs outside this
suite — see the tradeoffs table in the results directory.

### Added

- `.cavspatch` v2 (`CAVSPCH2`) for optimized pairwise sidecars over
  whole directory builds ([PAIRWISE_SIDECARS.md](docs/PAIRWISE_SIDECARS.md)).
- Per-file strategy selection across `copy-old`, streaming plan ops,
  `bsdiff`, `xdelta3` and `full-data` — every applicable candidate is
  generated and measured, the smallest payload wins;
  `--explain-strategies` writes the per-file reasoning.
- Auto compression selection between zstd-19 and brotli-9 per payload
  section, each with its own BLAKE3.
- `cavs route-plan` with device profiles (`default`, `low-memory`,
  `slow-network`, `low-disk`) and memory-budget filtering
  ([DELIVERY_PLANNER.md](docs/DELIVERY_PLANNER.md)).
- Hot-pair patch policy (`cavs patch-policy`, TOML config) to avoid
  O(N²) patch generation.
- `cavs publish-dir` for one-pass release publishing (container +
  signature + plan + sidecar + preview).
- Rename/move detection by content: zero-payload metadata in sidecars,
  reported in `cavs preview`.
- Compressed/high-entropy blob detection
  (`preview --detect-compressed-blobs`) and automatic byte-level-delta
  routing for such files in the sidecar optimizer.
- Full-pipeline benchmark suite (`cavs bench butler-full`,
  `cavs bench full-pipeline`) covering benchmarks A–H, with raw outputs
  under [docs/results/v0.8.0/](docs/results/v0.8.0/README.md).
- Journaled staged sidecar apply (`staging → verified → committing →
  committed`, `failed` on aborts) with recovery tests
  (`cavs test apply-recovery`).
- `--memory-budget` on `cavs apply-patch`; error codes
  `CAVS-E-PATCH-CORRUPT`, `CAVS-E-PATCH-INVALID`,
  `CAVS-E-MEMORY-BUDGET-EXCEEDED`, `CAVS-E-BUTLER-REDIFF-FAILED`.

### Changed

- `cavs optimize-patch` defaults to `--algo auto` / `--compression auto`
  and accepts directories; it now emits v2 sidecars (v1 files remain
  applicable via `cavs apply-patch`).
- Workspace version 0.8.0.

## [0.7.0]

The offline toolkit release. CAVS can now sign, preview, diff, apply, verify
and benchmark game-build updates **locally, with no CAVS server** — the same
verified copy-range + fresh-data reconstruction model the online client uses,
driven from the command line. This release also adds a fair external **butler
offline** benchmark harness and a multi-route benchmark suite so CAVS can be
compared honestly against full downloads, butler and pairwise delta tools.

Measured highlights (128 MiB synthetic builds, seed 5; butler v15.27.0,
xdelta3 3.2.0, bsdiff). On a typical directory-build release the offline
`.cavsplan` update is **2.51 MiB** — half the v0.6 chunk-route wire (5.42 MiB),
matching butler's 2.52 MiB while diffing 2× faster and applying with a
streaming 8 MiB memory budget instead of butler's 35 MiB (and bsdiff's
2.3 GiB). On a shifted artifact (every byte moved) CAVS ships **4.21 KiB** vs
butler's 68 KiB — 16× less — and ties the byte-level tools. The many-version
stream shows the store-once model: 10 versions of a 32 MiB build live in a
**30.6 MiB** content-addressed store that serves *any* jump directly, where
covering every pair with dedicated patches would need 45 of them. Every route
was verified byte-identical.

### Added

- **`cavs preview`** — classify a new build against the previous version's
  `.cavssig` as `NEW` / `MODIFIED` / `DELETED` / `SAME`, estimate the update
  cost per route, and warn when a large modified file looks
  compressed/high-entropy (small source changes cascade across compressed
  output — publish the folder instead). `--changes-only`, `--json`.
- **`cavs diff-plan`** — produce a deterministic, BLAKE3-sealed offline
  reconstruction plan (`.cavsplan`, `cavs-plan` crate): COPY ranges that
  reuse old bytes + INLINE data (zstd-19) for what changed, plus directory
  metadata and managed deletions. `portable` (self-contained patch) or
  `--analysis` (ops + estimates only); diffs against `--old-signature`
  without the old bytes present. Deterministic: same inputs ⇒ same bytes.
- **`cavs apply`** — execute a `.cavsplan` locally. Artifact plans write
  `<out>.part` and rename after a full-hash check; directory plans stage into
  `.cavs-staging/`, verify every file, journal intent, then commit per file.
  An interrupted apply finishes by re-running (or `--resume <journal>`);
  unchanged files are never touched (mtime survives), mods/saves are
  preserved, deletions happen only with `--delete-removed-files`. A failed
  apply never leaves corrupt output.
- **`cavs verify-install`** — verify an installed artifact/directory against a
  `.cavssig` or a manifest's `sha256:` digests, reporting exact
  `MODIFIED`/`MISSING`/`EXTRA` per entry and exiting non-zero on mismatch.
  `--allow-extra-files` tolerates mods and saves.
- **`cavs file` / `cavs ls`** — identify and list any CAVS file (`.cavs`,
  `.cavssig`, `.cavsplan`, `.cavspatch`, manifest, zstd bootstrap); unknown or
  corrupt files fail cleanly. `cavs signature ls` and `--json` on the
  signature commands.
- **`cavs bench butler-offline`** — drive an external `butler` binary through
  its `diff`/`apply`/`verify` pipeline (`-j` JSON lines captured), measure
  wall time and peak RSS, and verify the output byte-for-byte. Labeled as
  butler's **offline/default patch**, explicitly *not* the
  backend-optimized patch. Fails gracefully when butler is absent.
- **`cavs bench pairwise-proxy`** — approximate the optimized pairwise-patch
  class with bsdiff/xdelta3 × zstd/brotli, always labeled a **proxy**, never
  official backend numbers; records tool versions, verifies every apply.
- **`cavs bench routes`** — every delivery route for one transition in one
  table (full downloads, CAVS chunk/hybrid, CAVS offline plan, butler
  offline, pairwise proxies). Missing tools are skipped, not fatal.
- **`cavs bench version-stream`** — many-version storage/served-bytes
  comparison (CAVS store-once vs pairwise patches for adjacent updates, long
  jumps and reinstalls), and **`cavs bench gen-dir`** for synthetic directory
  build pairs (modified/new/deleted/renamed files).
- **Experimental pairwise sidecars** — `cavs optimize-patch` /
  `cavs apply-patch` wrap an external byte-level delta in a verified
  `.cavspatch` for one hot old→new pair (both ends BLAKE3-checked, atomic
  rename). Optional route with an explicit O(N²) warning.

### Changed

- **Directory/container mode is stable** (was a v0.6.0 preview):
  `--ignore <glob>` (repeatable) plus a root `.cavsignore`, path
  normalization and traversal rejection, deterministic sorted packing.
- 13 new stable `CAVS-E-*` codes: `PLAN-CORRUPT`, `PLAN-INVALID`,
  `APPLY-HASH-MISMATCH`, `JOURNAL-CORRUPT`, `JOURNAL-RESUME-FAILED`,
  `PATH-TRAVERSAL`, `UNSUPPORTED-SYMLINK`, `PAIRWISE-TOOL-MISSING`, and the
  `BUTLER-NOT-FOUND` / `-DIFF-FAILED` / `-APPLY-FAILED` / `-VERIFY-FAILED`
  harness codes.
- New crate `cavs-plan`; new docs: `OFFLINE_TOOLKIT.md`,
  `CAVSPLAN_FORMAT.md`, `DIRECTORY_MODE.md`, `ROUTE_BENCHMARKS.md`,
  `BUTLER_COMPARISON.md`, `PAIRWISE_SIDECARS.md`.

### Notes

The butler benchmark measures butler's offline/default patch, not the
backend-optimized player patch (bsdiff + high-quality Brotli). The
bsdiff/Brotli results are reported separately and labeled as an optimized
pairwise **proxy**, not official backend numbers. No wire format or routing
changed for existing paths; the v0.5/v0.6 online numbers are unaffected.

## [0.6.0]

The hybrid reconstruction release: CAVS can now use a previously installed
artifact or directory tree as a first-class source of reusable bytes, while
keeping the content-addressed cache, packfile store and verified,
byte-identical reconstruction model intact. It folds the core idea of delta
patching (old-version signatures, copy-range reuse, coalescing, preferred
sources, no-op detection, staged applies) into the CAVS model — as design
ideas, not a rewrite.

Measured highlights (128 MiB synthetic suite, seed 5): a client with an
**empty cache but the old version on disk** updates for 6.24 MiB instead of
64.55 MiB (−90.3 % small change), 26.53 MiB instead of 64.56 MiB (−58.9 %
medium), and 10.9 KiB instead of 64.56 MiB (−99.98 % shifted). The
warm-cache path is byte-for-byte unchanged from v0.5 (no regression), every
output stays byte-identical, and a corrupt previous install demotes to
cache/network per range instead of failing. Range coalescing turns 1,082
per-chunk ops into 18 contiguous reads on the shifted variant. A 128 MiB
build's `.cavssig` signature is 88 KiB (0.07 %). Re-fetching an
already-current install is a no-op: 0 payload bytes.

### Added

- **`--previous-artifact` client mode.** The old install is memory-mapped,
  chunked with the packer's recorded profile and indexed by the new
  manifest's hashes; matched ranges are copied directly — each one
  BLAKE3-verified before writing, with the final SHA-256 gate unchanged. A
  failed range demotes to cache/network and reports
  `CAVS-E-PREVIOUS-ARTIFACT-MISMATCH` (recoverable). The client also
  overrides a server bootstrap suggestion when the previous install makes
  the chunk path cheaper.
- **Hybrid reconstruction plans** (`cavs-rebuild-plan` crate). Every data
  track is rebuilt through a unified, deterministic plan
  (`CopyPreviousRange` / `CopyCacheChunk` / `FetchNetworkChunk`) with
  cost-based source scoring (network ≫ seeks ≫ local reads), contiguity
  preference, and strict adjacent-range coalescing up to 8 MiB per read.
  The v0.5 cache+network flow is expressible as a plan with no
  previous-range ops. `--dump-plan plan.json` exports it; `--no-hybrid`
  restores v0.5 behaviour.
- **CAVS signatures (`.cavssig`)** (`cavs-signature` crate) with
  `cavs signature export|inspect|verify`: a compact (0.07 % of source),
  deterministic description of an old artifact/directory — fixed 64 KiB
  blocks, weak rolling-hash prefilter, BLAKE3-256 strong hashes, per-file
  layout, Merkle root and a whole-file integrity trailer. Exportable from
  `.cavs` containers, raw files or directories.
  `cavs pack --against-signature old.cavssig` reports reusable bytes at
  pack time without the old content. New fuzz target
  (`fuzz_signature_decode`) asserts decode∘encode canonicality.
- **Hybrid diff scanner.** rsync-style weak rolling hash over new bytes,
  candidates confirmed with BLAKE3, `DATA` ops capped at 4 MiB, adjacent
  copies coalesced — finds shifted/unaligned reuse against a signature
  alone (no old bytes, no chunk cache).
- **No-op detection** (default on; `--force-reconstruct` disables): outputs
  that already match cost one manifest round-trip
  (`delivery_mode: "no-op"`); a previous artifact that already *is* the
  target installs by verified local copy (`"previous-copy"`); directory
  updates skip unchanged files (mod-friendly).
- **Directory/container mode (preview).** `cavs pack-dir ./Build -o b.cavs`
  packages a tree as per-file deduplicated tracks plus dir/symlink/exec
  metadata; the client reconstructs into a staging directory, verifies
  every file hash, commits with per-file renames under a journal, and
  optionally `--prune`s files dropped by the new version (unknown files —
  mods, saves — are preserved by default).
- **Delta benchmark baseline.** `cavs bench delta --old A --new B [--out d]`
  measures a block-based delta model (64 KiB blocks, weak rolling hash +
  BLAKE3 confirmation, COPY/DATA planning, zstd-1 transport) against full
  re-download, the CAVS chunk route, and xdelta3/bsdiff when installed —
  patch size, generation and apply times, plus JSON/markdown reports. See
  docs/DELTA_COMPARISON.md for results and honest framing (pairwise patches
  win per-pair bytes; CAVS wins the operational model).
- **Compression benchmark.** `cavs bench compression --input f --algos
  zstd-3,brotli-9` (Brotli feature-gated behind `brotli-bench`). Measured:
  zstd and Brotli within 0.1 % on size, zstd ~40× faster to decode — zstd-3
  stays the default.
- **Static/CDN export plans.** `cavs store export --out dir --static-plans`
  adds per-asset `chunk-map.json` (chunk → pack file, offset, lengths) so a
  client against a static HTTP host can plan fetches with no smart server.
- **Per-source fetch stats.** `--stats-json` now reports
  `sources.{network,cache_chunk,previous_artifact,repair_wire}_bytes`,
  demoted chunk counts, plan op counts before/after coalescing and
  source-selection time.
- **Error taxonomy additions** (stable codes): `CAVS-E-SIGNATURE-CORRUPT`,
  `CAVS-E-SIGNATURE-MISMATCH`, `CAVS-E-PREVIOUS-ARTIFACT-MISSING`,
  `CAVS-E-PREVIOUS-ARTIFACT-MISMATCH`, `CAVS-E-HYBRID-PLAN-INVALID`,
  `CAVS-E-HYBRID-SOURCE-FAILED`, `CAVS-E-CONTAINER-APPLY-FAILED`,
  `CAVS-E-CONTAINER-ROLLBACK-FAILED`, `CAVS-E-DELTA-BENCH-UNAVAILABLE`.

### Changed

- The client reconstruction path for container payloads (raw and directory)
  now goes through the unified plan executor; existing cache/network
  behaviour is preserved as a special case of the plan model (verified: the
  planner never increases network bytes over v0.5 for the same cache
  state). Media payloads keep the v0.5 streaming path.
- Bloom false-positive repair now also consults the previous-artifact index
  before re-fetching a referenced chunk.

### Notes

This release does not replace FastCDC or convert CAVS into a pairwise
patcher. It adds a hybrid source model: cache chunks, previous-artifact
ranges, packfile ranges and network chunks all participate in the same
verified rebuild. New docs: docs/HYBRID_RECONSTRUCTION.md,
docs/SIGNATURE_FORMAT.md, docs/DELTA_COMPARISON.md.

## [0.5.0]

The production-hardening release: correctness under malformed input, recovery
from interrupted downloads and corrupt caches, structured errors, fuzzing,
and large-build confidence. Not about reducing bytes — about trust. Wire
numbers are byte-for-byte identical to 0.4.0 on the real-game suite (tps-demo
update still 1.64 MiB, warm re-fetch still 0 bytes, everything
byte-identical), so all 0.3.0/0.4.0 wins carry over unchanged.

Measured highlights: an interrupted 232 MiB bootstrap download (client
killed with `kill -9` at 57 MiB) resumed with an HTTP Range request and paid
only the missing ~166 MiB; `cache verify` + `cache repair` on a real
5,747-chunk cache detected, quarantined and re-fetched exactly the corrupted
entries; client peak RSS stays ~14 MiB installing a 569 MB game; the 1 GiB
synthetic suite packs in ~7 s per version, and a head-insertion that shifts
every byte of a 1 GiB build costs 10.9 KiB of update egress (FastCDC
resynchronization working as designed).

### Added

- **Structured error taxonomy.** Stable `CAVS-E-*` codes
  (`cavs_proto::errors`): `MANIFEST-CORRUPT`, `BOOTSTRAP-HASH-MISMATCH`,
  `CHUNK-HASH-MISMATCH`, `CACHE-CORRUPT-RECOVERABLE`, `NETWORK`,
  `OUTPUT-HASH-MISMATCH`, `SIGNATURE-INVALID` and friends — attached at
  every client/CLI failure point, recoverable from any rendered error
  chain, so tooling can decide retry/repair/give-up without parsing prose.
- **Fuzzing.** Five libFuzzer targets under `fuzz/` (manifest v2, varint,
  pack index, container, CVSP batch; `cargo +nightly fuzz run <target>`),
  plus deterministic mini-fuzz replay tests that run in normal CI:
  full byte-flip sweeps, truncation sweeps and seeded random garbage
  against every decoder.
- **Corruption matrix.** `cavs test corrupt <file.cavs> [--out report.json]`
  mutates a scratch copy across ~20 targeted rows — container magic,
  section directory, section bytes, chunk data, truncation, manifest v2
  header/body/truncations, overlong/truncated varints, bootstrap sidecar,
  packfile header/data/footer, pack index, out-of-range reads — and
  asserts every decoder rejects the corruption cleanly.
- **Resume downloads.** A crash-safe journal (`<cache>/journal/…`, written
  tmp+rename) records in-flight fetches. Interrupted bootstrap downloads
  keep their `.zst.part` and continue with an HTTP `Range` request
  (`cavs-server` now answers 206 on `/bootstrap`; older servers get a
  clean restart); interrupted chunk fetches resume naturally from the
  cache have-set. Journals are honoured only when server, asset and
  manifest hash all match — anything stale is discarded with its partial
  files. New `cavs-client resume` command; `--no-resume` opts out. The
  final artifact is still only promoted after full verification.
- **Retry with backoff.** Transient failures (transport errors, 429/5xx)
  retry up to 5 times with exponential backoff (250 ms → 8 s, ±25%
  jitter); verification failures and 4xx never retry. Exhausted retries
  surface as `CAVS-E-NETWORK`.
- **Cache maintenance.** `cavs-client cache verify` re-hashes every cached
  chunk, quarantines corrupt entries (or `--delete`s them) and removes
  torn temp files; `cache repair <server> <asset>` re-fetches exactly the
  missing/corrupt chunks of an asset; `cache gc --max-size 10GiB` evicts
  least-recently-used chunks to a size budget.
- **`cavs doctor`.** Read-only diagnosis: a `.cavs` (structure, every
  chunk hash, Merkle root, manifest encodability in both formats,
  bootstrap sidecar size+BLAKE3, signature, duplicate chunk entries), a
  global store (`--store`: layout, ledger consistency, every chunk, pack
  integrity) and a client cache (`--cache`: corrupt entries). `CAVS-E-*`
  findings, non-zero exit on problems.
- **Large-build benchmark suite.** `cavs bench gen` produces deterministic
  synthetic datasets (same seed ⇒ identical bytes anywhere): a base build
  plus small/medium/large-change, head-shifted and reordered update
  variants, streamed so datasets larger than RAM generate fine.
  `cavs bench suite` packs every version and reports pack time, container
  and manifest sizes, dedup, update egress and packfile shape as
  `summary.md` + `summary.json`.

### Fixed

- **Unbounded pre-allocation in the CVSP decoders** (found by the new
  fuzz targets): a crafted batch header declaring billions of items or a
  multi-GiB inline payload could force huge allocations before the read
  failed. Counts are now capped by what the buffer could actually encode,
  and inline lengths are validated against a 256 MiB wire ceiling before
  allocating.
- **Container reader accepts less.** The superblock's declared hash
  algorithm, compression algorithm and file size are now validated
  (values a correct writer never produces are rejected). The remaining
  superblock fields (uuid, timescale, flags) are intentionally
  unauthenticated metadata — content integrity is carried by the section
  hashes, chunk hashes and Merkle root, as verified by the new full
  byte-flip sweep.

## [0.4.0]

The packfile release: the global store can now keep its chunks in a few
large immutable `.cavspack` files instead of one file per chunk, served by
coalesced range reads. Loose stores keep working unchanged; `.cavs`
file-serving is untouched.

Measured on real Godot games (two versions ingested, full
cold + update + warm HTTP session): chunk objects on disk drop from 130/807/
5,775 (Marble/GDQuest/tps-demo) to **4/4/6 files**, and physical reads
coalesce **65×/115×/170×** with **1.000 read amplification** (zero extra
bytes read — chunks are packed in reconstruction order, so merged ranges are
exactly contiguous). Wire bytes, routing and byte-identical reconstruction
are identical to 0.3.0 in every layout and in `.cavs` file-serving mode.

### Added

- **Packfile storage** (`cavs store add --storage packfiles`). Chunks are
  appended in reconstruction order into content-addressed packs
  (`packs/<ab>/<id>.cavspack`, id = BLAKE3 of the file) with a verifiable
  `.cavsindex` sidecar each. The layout is fixed at store creation; the
  ledger records each chunk's pack and offset. GC deletes a pack once no
  live chunk references it (the roadmap's zero-live-pack policy).
- **Coalesced range serving.** The server plans each batch's cold chunks
  as one read set: chunks from the same pack within a 64 KiB gap are
  fetched with a single physical read (capped at 8 MiB). New metrics:
  `cavs_pack_chunks_requested_total`, `cavs_pack_ranges_read_total`,
  `cavs_pack_bytes_read_total`, `cavs_pack_bytes_served_total`.
- **Manifest chunk-location hints.** Binary v2 manifests of packfile-store
  assets carry an optional ChunkLocations section (section kind 4 —
  skipped by 0.3.0 readers) mapping each chunk to `pack_id + offset +
  stored_len`. Advisory: consumers verify by BLAKE3 regardless.
- **`cavs store export --out`** — deterministic immutable object tree
  (`chunks/packs/…`, `chunks/indexes/…`, `assets/<name>/record.json`)
  ready to upload to S3/R2/a static host behind a CDN, with the cache
  headers to use.
- **`cavs store verify`** — re-hashes every chunk (loose or packed,
  including zstd-stored) and checks pack header/footer integrity.
- **ETag headers** (`"blake3-…"`) on the immutable chunk and bootstrap
  endpoints, complementing the existing immutable Cache-Control.

## [0.3.0]

The compact-manifest release: the runtime manifest now travels as a compact
binary format (`CAVSMF2`) instead of JSON, cutting manifest wire overhead
dramatically while keeping full JSON v1 compatibility for old clients and
servers. Reconstruction, dual-route behavior and warm-cache savings are
unchanged.

Measured on real Godot games (64 KiB CDC, real HTTP sessions): manifests
shrink 75–77% — tps-demo 894 → 209 KiB, GDQuest 103 → 25 KiB, Marble
20 → 5 KiB — with parse time at parity with JSON. Chunk-path bytes are
byte-for-byte identical to 0.1.2 (tps-demo update still 1.64 MiB), warm
re-fetch stays at 0 payload bytes and reconstruction stays byte-identical.
Since the manifest is the dominant cost of an update check, a warm re-fetch
now costs ~75% less wire; total update egress improves up to −26.6%
(tps-demo) depending on how much the manifest weighed.

### Added

- **`cavs-manifest` crate.** One home for manifest wire formats: a strict
  unsigned-LEB128 varint codec, the binary v2 encoder/decoder, and
  `read_manifest`, which detects JSON v1 vs binary v2 from the bytes and
  normalizes both into the same runtime `Manifest` — server, client and CLI
  never branch on formats downstream.
- **Binary manifest v2 (`CAVSMF2`).** Sectioned envelope (AssetInfo,
  ChunkPlan, ChunkDictionary) with per-section BLAKE3 integrity. Chunk
  hashes are stored once, as raw 32-byte BLAKE3, in a dictionary; every
  chunk reference in the plan is a varint dictionary index instead of a
  repeated 64-char hex string. Sections ≥ 32 KiB are zstd-compressed. The
  decoder enforces hard limits (section count/size, decompression ratio,
  string length, overlong varints) so malformed or hostile manifests fail
  cleanly — verified by truncation sweeps and a full byte-flip test.
- **Format negotiation.** `GET /api/assets/{asset}/manifest` serves binary
  v2 when requested via `Accept: application/vnd.cavs.manifest-v2` or
  `?format=binary-v2`; JSON v1 remains the default response, so v0.2.x
  clients work unchanged. New per-format manifest counters in `/metrics`.
- **Client negotiation + manifest metrics.** `cavs-client` asks for binary
  v2 (JSON fallback keeps old servers working) and reports
  `manifest.format/wire_bytes/parse_ms/chunk_count_logical/chunk_count_unique`
  in `--stats-json`.
- **`cavs manifest export`** — readable JSON v1 manifest from a `.cavs`
  (debug/compatibility view).
- **`cavs manifest bench`** — compares JSON v1 vs binary v2 for the same
  container: wire bytes, parse time, bytes per logical chunk, savings; text
  and `--json` output.

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

[Unreleased]: https://github.com/orelvis15/cavs-oss/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/orelvis15/cavs-oss/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/orelvis15/cavs-oss/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/orelvis15/cavs-oss/compare/v0.1.2...v0.3.0
[0.1.2]: https://github.com/orelvis15/cavs-oss/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/orelvis15/cavs-oss/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/orelvis15/cavs-oss/releases/tag/v0.1.0
