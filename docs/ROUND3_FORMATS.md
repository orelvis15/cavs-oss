# Round 3 formats — metadata packs, segmented index, chunk-map runs

This document specifies the on-disk and on-wire formats introduced by
Round 3 (metadata & many-files, segmented index, run-encoded chunk maps),
their compatibility story, and the operational commands around them.

## 1. Session meta-packs (Round 3A)

**Problem.** Every fetched object cost two serialized metadata round-trips
(`assets/<oid>/manifest.json` + `assets/<oid>/chunk-map.json`). On a WAN,
a many-object clone spends most of its wall time in those round-trips.

**Layout in the export tree:**

```text
meta/
  index.json               oid -> meta-pack mapping (mutable, atomic swap)
  packs/<id>.cmeta         one immutable pack per publish session
```

- `<id>` is the BLAKE3 hex of the pack's compressed bytes
  (content-addressed, safe to cache forever:
  `Cache-Control: public, max-age=31536000, immutable`).
- `.cmeta` = zstd-compressed JSON:

```json
{"version": 1, "objects": [
  {"oid": "<sha256>", "manifest": { ... }, "runs": [ ... ]}
]}
```

- `meta/index.json`:

```json
{"version": 1, "generation": 3, "packs": [
  {"id": "<blake3>", "oids": ["<sha256>", "..."]}
]}
```

Later packs win when an oid appears twice (re-push). When the index is
missing or unreadable, the publisher rebuilds it by scanning
`meta/packs/*.cmeta` (packs are the source of truth; the index is a
derived accelerator).

**Client resolution order** (`cavs_fetch::MetadataResolver`):

1. **L1** in-process cache (session-wide).
2. **L2** disk cache (`<cache>/meta/<ab>/<oid>.meta.zst`) — only served
   when the remote's index still maps the oid to the pack the entry came
   from, so a re-push or repack can never serve stale locations.
3. **Meta-pack route**: fetch `meta/index.json` once per session, then the
   pack holding the oid; every sibling object in the pack is prefetched
   into L1+L2 (this is what collapses a 250-object clone's metadata to a
   handful of requests).
4. **Fallback**: the classic per-asset `manifest.json` + `chunk-map.json`
   pair — used for remotes without meta-packs (negative-cached probe, 5 s
   TTL) and for oids missing from the index.

Concurrent resolves of one oid share a single network fetch
(singleflight). Stats: `MetaStats` (requests, l1/l2 hits, pack fetches,
prefetched, fallbacks, negative hits).

**Server plane**: `POST /v1/metadata/batch` on cavs-server accepts up to
128 `{oid}` objects and answers per-object
(`available` + manifest + locations | `missing`) — partial responses,
never all-or-nothing. Metrics: `cavs_metadata_batch_requests_total`,
`cavs_metadata_batch_objects_total`.

## 2. Chunk-map v2 by runs (Round 3B)

Inside meta-packs, object locations are **run-encoded**: physically
contiguous chunks of the same pack state the pack path and start offset
once; per-chunk offsets are implicit (cumulative `len_stored`).

```json
{"pack": "chunks/packs/ab/<id>.cavspack", "start_abs": 16,
 "hashes": ["...", "..."], "lens_raw": [..], "lens_stored": [..],
 "flags": 3}
```

`flags` is a single integer when uniform across the run (the common
case), else an array. A push writes an object's chunks contiguously, so a
many-chunk object typically serializes as a handful of runs — >30% fewer
metadata bytes than the per-chunk v1 encoding (enforced by test).

**Compatibility:** readers prefer `runs` and fall back to a `chunks`
array (v1 entries) in the same object; the per-asset
`assets/<oid>/chunk-map.json` stays v1 so pre-Round-3 clients keep
working against Round-3 trees.

## 3. Segmented index (Round 3B)

**Problem.** `index.bin` must be read/rewritten whole: ~72 B/chunk means
~720 MB at 10 M chunks, per open and per save.

**Layout** (opt-in via `cavs store <dir> index-migrate`):

```text
<store>/index/
  CURRENT                          "gen-0000000042\n" (atomic swap)
  wal.log                          begin/commit journal of swaps
  segments/<seal>.seg              immutable, content-addressed pool
  generations/gen-N/root.idx       segment list + checksum
  generations/gen-N/assets.json.zst  asset -> chunk hexes
```

**Segment file** (`CAVSSEG1`, little-endian):

```text
0   magic "CAVSSEG1"
8   version u16 = 1
10  kind u8 (0 base | 1 delta), reserved u8
12  record_count u64
20  records: count x 73 B, sorted by hash
      hash[32] | len_raw u32 | len_stored u32 | flags u32 | refcount u64
      | zero_since u64 (MAX=none) | pack_ord u32 (MAX=none)
      | pack_offset u64 (MAX=none) | state u8 (0 live | 1 tombstone)
    pack table: u32 count, then {u16 len, hex} per pack id
tail  BLAKE3 seal (32 B) over everything above
```

- Lookups are `mmap` + binary search (fixed stride); the chunk table
  never loads into RAM. Base segments target ~64 MiB.
- A publish session commits its touched records (tombstones included) as
  **one delta segment** plus a new `root.idx` + `CURRENT` swap — the
  ledger is never rewritten whole.
- Deltas fold into fresh base segments when >8 accumulate or delta bytes
  exceed 25% of base bytes (copy-on-write compaction).
- Generations: current + previous retained (mirror of
  `index.bin.prev`); older generations and unreferenced pool segments are
  pruned. Generation dirs newer than `CURRENT` (crash between WAL
  `begin` and the swap) are swept on open.
- Corruption is detected **per segment** (`store verify` /
  `index-inspect`); a bad segment is named, not silently zeroed.

**Migration**: `index-migrate` is explicit and one-way; `index.bin` is
kept as `index.bin.pre-migration` (rollback = delete `index/`, rename it
back). Non-migrated stores keep the monolithic path untouched.

Scale probe (release, Apple Silicon): 1 M chunks — create + open + 1000
random lookups in ~1.6 s total, open alone sub-second (asserted in
`index_scale_segmented_1m_chunks`).

## 4. Adaptive concurrency (Round 3C)

`FetchOptions.connections == 0` (the new agent default) enables an AIMD
controller: min 2, initial 8, max 64; +1 connection per clean 1 s window,
halved (1 s cooldown) on pressure — failed range attempts, short reads,
HTTP 429/503. `CAVS_FETCH_CONCURRENCY=auto|N` overrides at deploy time;
`connections >= 1` keeps the historical fixed pool. The global
inflight-byte budget (`CAVS_FETCH_MAX_INFLIGHT_BYTES`) still applies.
`FetchStats` reports `concurrency_mode`, `concurrency_peak`,
`aimd_decreases`.

**Cache layering** after Round 3: L1 metadata (in-process), L2 metadata
(disk, generation-validated), decoded-chunk cache (the existing
content-addressed `ChunkCache` — never stale by construction). A raw
range cache (L3) was evaluated and deferred: the chunk cache already
covers warm/branch-switch reuse without staleness risk.

## 5. Fragmentation & repack (Round 3D)

`cavs store <dir> fragmentation` reports per-pack live/dead bytes,
small-pack ratio (<8 MiB), and a comparative score
(small-pack ratio + dead-bytes ratio, range [0,2]).

`cavs store <dir> repack [--dry-run]`:

1. plan: merge groups of small packs up to the preferred pack size;
   compact packs with >30% dead bytes;
2. execute copy-on-write: live chunks are rewritten into fresh packs
   (physical order preserved), the ledger swaps generation, and only then
   are old packs **quarantined** (recoverable for the quarantine window —
   the same two-stage deletion GC uses). Reads keep working throughout; a
   crash at any point loses nothing.

After repacking a store that backs an exported static tree, re-export the
affected assets (their chunk locations changed) — the publisher's next
`export_asset` + meta-pack refresh does this.
