# cavs-server — origin server

The `cavs-server` binary serves `.cavs` assets (or a global store) over
HTTP/HTTPS, sending each client only the chunks it lacks.

## How it works

On session open, the client announces its **have-set** (the chunks already in
its cache), either as an exact hash list or a compact Bloom filter for large
caches. For each requested asset the server plans delivery **per chunk**: send
a *reference* if the client already has it, or *inline* the payload (exactly as
stored — no recompression) if not. Chunks are content-addressed and immutable,
so the chunk endpoint is CDN-cacheable.

**Dual route (v2)**: if a `.cavs` was packed with `--bootstrap`, the server
verifies the `<asset>.cavs.bootstrap.zst` sidecar at load (size + BLAKE3
against the container metadata) and, at session open, estimates the chunk-path
payload for that client. A cold client (<5% of chunks cached) is routed to the
bootstrap whenever it is ≥2% cheaper — one immutable, CDN-cacheable download
at full-artifact price. A missing or tampered sidecar simply disables the
route; the chunk path always remains valid.

## Run

```sh
# Serve one or more .cavs files (asset name = file stem)
cavs-server game_v1.cavs game_v2.cavs --listen 127.0.0.1:8990

# Serve from a global store (chunks deduplicated across all versions on disk)
cavs-server --store ./store --listen 0.0.0.0:8990

# HTTPS
cavs-server *.cavs --listen 0.0.0.0:8990 --tls-cert cert.pem --tls-key key.pem
cavs-server *.cavs --tls-self-signed ./tls    # dev: generates a self-signed cert
```

## HTTP surface

| Endpoint | Purpose |
|---|---|
| `POST /api/assets/{asset}/sessions` | Open a session with a have-set (list or Bloom); the response carries the delivery route decision |
| `POST /api/sessions/{id}/batch` | Request tracks/segments; returns a binary CVSP batch |
| `GET /api/assets/{asset}/manifest` | Signed manifest (chunk table, Merkle root, signature). JSON v1 by default; compact binary v2 via `Accept: application/vnd.cavs.manifest-v2` or `?format=binary-v2` |
| `GET /api/assets/{asset}/bootstrap` | Full bootstrap artifact (whole asset, zstd) — immutable, CDN-cacheable, streamed from disk |
| `GET /api/assets/{asset}/chunks/{hash}` | Direct chunk fetch — immutable, CDN-cacheable |
| `GET /hls/{asset}/{track}/…` | HLS/CMAF reconstructed on the fly (video assets) |
| `GET /metrics` | Prometheus counters (inline/ref bytes, sessions, …) |

## Options

- `--max-cold N` — collapse policy: a segment with more than N cold chunks is
  delivered as a self-sufficient bundle.
- `--web-wasm <path>` — serve the browser player's WASM module (built separately).

Run `cavs-server --help` for all options.
