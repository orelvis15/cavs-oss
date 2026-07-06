# Architecture

CAVS is a content-addressable delivery layer that sits above your existing
formats. The core idea: split content into chunks identified by their hash,
store each unique chunk once, and transmit only the chunks a client lacks.

```
 build / CI                origin server              client (runtime)
┌────────────────────┐   ┌────────────────────┐   ┌────────────────────────────┐
│ cavs pack          │   │ cavs-server        │   │ native client / Godot / web │
│  build.pck→.cavs   │ → │  .cavs or --store  │ → │  persistent chunk cache     │
│ cavs store add     │   │  session have-set  │   │  fetch only missing chunks  │
│  (dedup at rest)   │   │  inline/ref plan   │   │  verify + atomic rebuild    │
└────────────────────┘   └────────────────────┘   └────────────────────────────┘
```

## Crates (`core/`)

| Crate | Responsibility |
|---|---|
| `cavs-hash` | BLAKE3-256 chunk identity, incremental hasher, binary Merkle root, the content-signature message |
| `cavs-chunker` | Chunking: fixed-size (CDN-aligned segments) and FastCDC (shift-resistant, default 64 KiB for game assets) |
| `cavs-store` | In-memory dedup index used while packing, and the **global content-addressable store** (on-disk CAS with reference counting and GC) |
| `cavs-format` | The `.cavs` binary format: types, streaming writer, hardened reader/verifier, Ed25519 signing |
| `cavs-proto` | CVSP wire protocol: the runtime `Manifest` model, sessions, compact binary batches; the Bloom-filter have-set |
| `cavs-manifest` | Manifest wire formats: the compact binary v2 codec (`CAVSMF2`: chunk dictionary + varint plan, ~76% smaller than JSON) and `read_manifest`, which detects JSON v1 vs binary v2 from the bytes and normalizes both |
| `cavs-cli` | The `cavs` binary: pack / unpack / info / verify / keygen / store / play / sweep, plus the payload classifier and chunk-profile cost model |
| `cavs-server` | Stateful HTTP/HTTPS origin: sessions, inline/ref planning, `--store` mode, HLS passthrough, metrics |
| `cavs-client` | Native streaming client: persistent cache, `.part`→verify→rename reconstruction |

The Godot plugin (`godot-plugin/`) is a pure-GDScript client — no native
binary — and the SteamPipe analyzer (`steam-analyzer/`) is a standalone tool
that reuses `cavs-hash` and `cavs-chunker`.

## How an update flows

1. **Pack once (publisher).** `--profile auto` classifies the payload (magic
   bytes, sampled entropy, a zstd probe) and measures candidate chunk profiles
   on the real bytes; engine packs default to FastCDC ~64 KiB chunks
   identified by BLAKE3. Chunks are stored deduplicated and zstd-compressed in
   a signable `.cavs`; `--bootstrap` additionally emits the whole artifact as
   one zstd-19 sidecar for cold installs. Unchanged regions across versions
   produce identical chunks (deduplicated for free); changed regions produce
   new chunks. Packing the next version with `--prev <published .cavs>` keeps
   the profile consistent with what clients already cached.

2. **The client announces what it has.** It first fetches the asset manifest —
   negotiated as compact binary v2 (`CAVSMF2`, ~76% smaller than the JSON v1
   equivalent) with JSON as the compatibility fallback — then opens a session
   sending the have-set of its persistent cache: either an exact hash list, or
   a compact Bloom filter for large caches. The server decides per chunk: send
   a *reference* if the client already has it, or *inline* the payload if not.

3. **The server picks the cheapest route (dual delivery).** At session open it
   estimates the chunk-path payload for this specific client. A cold client
   (<5% of chunks cached) whose estimate is beaten by ≥2% by the bootstrap
   artifact is routed to it: one immutable, CDN-cacheable download at
   full-artifact price. Everyone else gets the chunk path. The routing is
   advisory and the chunk path always remains valid.

4. **Only the new bytes travel.** Binary CVSP batches carry chunks exactly as
   stored (already compressed — zero recompression), decoded incrementally from
   the socket so peak memory is one chunk. A bootstrap download streams to
   disk the same way.

5. **Atomic, verified reconstruction — and cache seeding.** The client writes
   a `.part` temp file, verifies the manifest's SHA-256 on the fly, then
   atomically renames into place. A bootstrap install is additionally sliced
   along the manifest's chunk plan (each slice BLAKE3-verified) straight into
   the local cache, so the *next* update only pays for changed chunks. An
   interrupted download never leaves a corrupt file, and retrying only fetches
   what's missing.

## Integrity chain

- **Per chunk**: BLAKE3 of the decompressed payload must equal its identity hash.
- **Per section**: BLAKE3 of each table against the section directory.
- **Global**: Merkle root over the chunk table, checked against the INTEGRITY
  section; optionally an Ed25519 content signature the client can pin to a
  trusted publisher key.
- **Per file (thin clients)**: a SHA-256 per reconstructed file, embedded in
  the manifest, so clients without BLAKE3 (e.g. Godot) verify with a built-in
  hasher.

## Storage vs egress dedup

Client-side caching already delivers **egress** savings across versions and
sessions. The **global store** (`cavs store` / `cavs-server --store`) adds
**at-rest** savings: one physical copy of each unique chunk across every
published version and title, with reference counting and garbage collection.

## Design stance

CAVS is complementary to codecs, compressors and delta tools — not a
replacement. Dedicated pairwise deltas (xdelta, bsdiff) win on raw bytes for a
single version pair, but require O(N²) precompute across many live versions,
heavy RAM for large files, and don't provide resumable, CDN-cacheable,
cross-version reuse. CAVS packages once per release and the same chunk store
serves any version jump. See [`BENCHMARKS.md`](BENCHMARKS.md).
