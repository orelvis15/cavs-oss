# Git LFS transfer agent

v1.5.0: `cavs-lfs-agent` plugs CAVS into Git LFS as a [standalone custom
transfer agent](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md),
replacing LFS's whole-file transfer and storage with CAVS chunk-level dedup.
Git and your git host keep doing what they are good at (source, history,
collaboration); CAVS moves the heavy binaries.

## Why

Git LFS stores and transfers every version of a large file **whole**. CAVS
chunks files with FastCDC, so consecutive versions share almost all of their
chunks:

| | vanilla Git LFS | CAVS agent |
|---|---|---|
| push a 22 MiB file with ~3 MiB changed | 22 MiB stored + sent | **~3.3 MiB** stored + sent |
| pull that version on a machine that has the previous one | 22 MiB | **~0.5 MiB** (cache reuse) |

(Numbers from `core/cavs-lfs-agent/e2e/run.sh`, reproducible.)

## How it works

```
git push                                   git pull / clone
   │                                          │
   ▼ NDJSON (stdin/stdout)                    ▼
cavs-lfs-agent  upload:                    cavs-lfs-agent  download:
  pack blob → 1 raw track .cavs              fetch_static(remote, oid)
  ingest into <remote>/.store  (dedup!)      missing-set vs local chunk cache
  export static tree           (manifest,    concurrent range GETs, BLAKE3 +
  chunk-map, immutable packs)                sha256(oid) verified → tmp file
   │                                          │
   ▼                                          ▼
<remote>/assets/<oid>/… + chunks/packs/…   git-lfs moves file into .git/lfs
```

- The **asset name and track name are the LFS oid** (sha256), and the packed
  container carries `sha256:<oid> = <oid>` meta — so `cavs-fetch`'s existing
  end-to-end verification checks the LFS oid for free on every download.
- Uploads land in one **shared `GlobalStore`** at the remote: chunks dedup
  across all objects, versions and (on a shared filesystem) all pushers. A
  flock on `<tree>/.store.lock` serializes writers; the static export runs
  *before* `complete` is reported, so a pushed object is always fetchable.
- Downloads run through the embeddable `cavs-fetch` engine: local CAS cache
  keyed by BLAKE3, only missing chunks travel, `--connections` parallel
  ranges, optional `--pubkey` Ed25519 manifest enforcement.

## Setup

See the crate README (`core/cavs-lfs-agent/README.md`) for the full setup,
clone-time config, options table and limitations. The short version:

```sh
git config lfs.standalonetransferagent cavs
git config lfs.customtransfer.cavs.path /path/to/cavs-lfs-agent
git config lfs.customtransfer.cavs.concurrent false
```

## CDN story

The tree the agent maintains at a directory remote *is* a CAVS static export
(same layout as `cavs store export --static-plans`). To serve clones from a
CDN:

```sh
# push over the directory remote as usual, then:
rclone sync /srv/lfs-cavs r2:my-bucket/lfs   # or aws s3 sync
# clients that only pull:
git config lfs.customtransfer.cavs.args "--remote https://cdn.example.com/lfs"
```

Packs are immutable and content-addressed — serve them with
`Cache-Control: public, max-age=31536000, immutable`.

## Troubleshooting

- `GIT_TRACE=1 git push` shows the exact agent invocation and every protocol
  event; the agent logs diagnostics to stderr with `[lfs-agent]` prefixes.
- Pointer files in the working tree after clone → the LFS filter was not
  configured at clone time (see clone-time `-c` flags in the README).
- `upload … remote is read-only (http)` → uploads need a directory remote;
  HTTP remotes are for downloads.
- A crash mid-push cannot corrupt: chunk/pack writes are atomic, and the next
  push of the same oid repairs the export if it is missing.
