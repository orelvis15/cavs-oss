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
| `POST /api/assets/{asset}/sessions` | Open a session with a have-set (list or Bloom) |
| `POST /api/sessions/{id}/batch` | Request tracks/segments; returns a binary CVSP batch |
| `GET /api/assets/{asset}/manifest` | Signed manifest (chunk table, Merkle root, signature) |
| `GET /api/assets/{asset}/chunks/{hash}` | Direct chunk fetch — immutable, CDN-cacheable |
| `GET /hls/{asset}/{track}/…` | HLS/CMAF reconstructed on the fly (video assets) |
| `GET /metrics` | Prometheus counters (inline/ref bytes, sessions, …) |

## Options

- `--max-cold N` — collapse policy: a segment with more than N cold chunks is
  delivered as a self-sufficient bundle.
- `--web-wasm <path>` — serve the browser player's WASM module (built separately).

Run `cavs-server --help` for all options.
