# CAVS core

The delivery engine: a Cargo workspace of Rust crates that implement the
`.cavs` format, the content-addressable store, the CVSP protocol, and the
three command-line tools (`cavs`, `cavs-server`, `cavs-client`).

## How it works

Content is split into **chunks** identified by their BLAKE3-256 hash. Each
unique chunk is stored once; a client downloads only the chunks it doesn't
already have, verifies them, and reconstructs the original files
byte-for-byte. See [`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) for the
full picture and [`../docs/FORMAT.md`](../docs/FORMAT.md) for the byte layout.

## Crates

| Crate | Kind | Role |
|---|---|---|
| [`cavs-hash`](cavs-hash) | lib | BLAKE3-256 chunk identity, Merkle root, signature message |
| [`cavs-chunker`](cavs-chunker) | lib | Fixed-size and FastCDC chunking |
| [`cavs-store`](cavs-store) | lib | Dedup index + on-disk global content-addressable store (refcount + GC; loose or `.cavspack` packfile layout with coalesced range reads) |
| [`cavs-format`](cavs-format) | lib | The `.cavs` binary format: writer, hardened reader, Ed25519 signing |
| [`cavs-proto`](cavs-proto) | lib | CVSP wire protocol: manifests, sessions, binary batches, Bloom have-set, `CAVS-E-*` error taxonomy |
| [`cavs-manifest`](cavs-manifest) | lib | Manifest wire formats: compact binary v2 (`CAVSMF2`) codec + JSON v1 compatibility reader |
| [`cavs-cli`](cavs-cli) | **tool** `cavs` | Package / inspect / verify / reconstruct / manage the store / doctor / corruption matrix / bench suite |
| [`cavs-server`](cavs-server) | **tool** `cavs-server` | Stateful HTTP/HTTPS origin (Range-resumable bootstrap endpoint) |
| [`cavs-client`](cavs-client) | **tool** `cavs-client` | Native streaming client: persistent cache with verify/repair/gc, resume journal, retry with backoff |

## Build and test

```sh
cargo build --release      # binaries in ../target/release/
cargo test                 # unit + integration + end-to-end
```

## Use

```sh
# package two builds, serve them, update a client with only the changed chunks
cavs pack --raw game_v1.pck -o game_v1.cavs
cavs pack --raw game_v2.pck -o game_v2.cavs
cavs-server game_v1.cavs game_v2.cavs --listen 127.0.0.1:8990
cavs-client fetch http://127.0.0.1:8990 game_v1 -o out1 --cache ./cache
cavs-client fetch http://127.0.0.1:8990 game_v2 -o out2 --cache ./cache
```

Each tool has its own README with full options: [`cavs-cli`](cavs-cli/README.md),
[`cavs-server`](cavs-server/README.md), [`cavs-client`](cavs-client/README.md).
