# cavs-lfs-agent benchmark — plain git vs Git LFS vs Git LFS + CAVS

Measured 2026-07-21 on macOS (Apple Silicon, APFS, local filesystem
remotes), git 2.50.1, git-lfs 3.7.1, release builds. Reproduce with
`bench/run.sh` — datasets are deterministic (seeded), raw metrics in
[results/results.csv](results/results.csv), CAVS store stats in
`results/*-store-stat.txt`.

These are the measurements the CAVS Cloud MVP spec asks for (§17):
Git LFS compatibility, storage efficiency vs Git LFS, reconstruction
correctness, and benchmark reports with the dashboard metrics (logical vs
physical storage, dedup %, objects, unique chunks).

## Systems

| | remote | what the remote stores |
|---|---|---|
| **git** | bare repo, `gc --aggressive` after every push | packfiles, binary deltas |
| **lfs** | vanilla Git LFS, `file://` standalone transfer | every object version, whole |
| **cavs** | `cavs-lfs-agent`, directory remote | shared chunk store **+ static export** (2 copies by design; a cloud backend would hold 1 — both numbers reported) |

All three ran the same protocol: push v1…vN from a working repo; after every
push, a tracking clone pulls and its **entire tree is sha256-verified**
against the source dataset; finally a cold clone at vN is verified too.
**All 12 scenario×system combinations verified byte-identical at every
step (44 pushes, 44 verified checkouts).**

## Scenarios (deterministic, `bench/gen.py`)

| | shape | versions | change per version |
|---|---|---|---|
| big-binary | one 100→104 MiB incompressible asset | 5 | ~1.25 MiB edited + 1 MiB appended |
| compressible | one 64 MiB semi-structured pack (~2× zstd) | 4 | ~2 % of blocks replaced |
| many-files | 250 files, log-normal sizes, 86.5 MiB | 4 | 10 % of files partially edited |
| full-rewrite | one 48 MiB blob | 2 | 100 % rewritten (worst case) |

## Storage at the remote (KiB, after last version)

| scenario | logical (latest) | git | vanilla LFS | **CAVS total** | **CAVS single-copy**¹ | CAVS vs LFS (total / single) |
|---|---:|---:|---:|---:|---:|---|
| big-binary | 106 496 | 111 736 | 522 267 | **241 140** | **119 268** | **−54 % / −77 %** |
| compressible | 65 536 | 20 128 | 262 171 | **66 129** | **32 753** | **−75 % / −88 %** |
| many-files | 88 527 | 96 902 | 114 392 | 212 488 | 105 874 | +86 % / −7 % |
| full-rewrite | 49 152 | 98 362 | 98 330 | 198 290 | 99 034 | +102 % / +1 % |

¹ chunk store only (`.store/`) — what a CAVS Cloud backend would bill; the
directory remote additionally keeps the CDN-syncable static export.

**Storage growth per pushed version** (the steady-state cost of an update):

| scenario | git | vanilla LFS | CAVS (single-copy) |
|---|---:|---:|---:|
| big-binary | +2.3 MiB | +101–104 MiB | **+4.3 MiB** |
| compressible | +0.4 MiB | +64 MiB | **+4.7 MiB** |
| many-files | +2.8 MiB | +8.5 MiB avg | +5.5 MiB avg |
| full-rewrite | +48 MiB | +48 MiB | +48.4 MiB |

CAVS store dedup stats (the spec's dashboard metrics):

| scenario | objects | unique chunks | logical → stored | dedup |
|---|---:|---:|---|---:|
| big-binary | 5 | 1 492 | 510.0 → 114.5 MiB | **77.5 %** |
| compressible | 4 | 726 | 72.3 → 31.4 MiB | **56.6 %** |
| many-files | 325 | 1 496 | 111.6 → 102.1 MiB | 8.5 % |
| full-rewrite | 2 | 1 278 | 96.0 → 96.0 MiB | 0 % |

## Download (tracking clone pulling each new version)

Bytes added to the clone's object/chunk cache per update (CAVS = raw chunk
cache growth — an upper bound on wire bytes, since the wire is
zstd-compressed):

| scenario | vanilla LFS per update | CAVS per update | saving |
|---|---:|---:|---:|
| big-binary | 101–104 MiB (whole file) | **3.5–3.9 MiB** | **−96 %** |
| compressible | 64 MiB | ≤ 16 MiB raw (≈ 5 MiB wire) | −75…−92 % |
| many-files | 6–11.4 MiB | **4.5–6.1 MiB** | −37 % |
| full-rewrite | 48 MiB | 48 MiB | 0 % |

Cold clone at the latest version downloads the logical size in all three
systems (git additionally carries the full history — its cold clone of
big-binary is 111.7 MiB vs 106.5 for LFS/CAVS).

Plain-git per-update wire is not comparable in this harness: over the local
path transport git transferred full packs (~full file per pull) even though
its *storage* deltas are excellent; a real smart-HTTP server negotiates thin
packs. Its storage column is the honest number here.

## Wall time (total push time, all versions)

| scenario | git | vanilla LFS | CAVS |
|---|---:|---:|---:|
| big-binary | 19.9 s | 14.1 s | **12.3 s** |
| compressible | 7.0 s | 10.2 s | 8.3 s |
| many-files | 4.9 s | 18.8 s | 42.4 s |
| full-rewrite | 6.0 s | 8.1 s | 6.7 s |

Update/clone times were within ~±1 s of vanilla LFS everywhere (local
filesystem; on a real network the wire savings dominate).

## Findings

**Where CAVS wins (the target use case).** Large binary assets that evolve
— the game-asset shape — get **−77 % to −88 % storage vs Git LFS
(single-copy) and −96 % update download** on the big-asset scenario, at
equal or better push time. This is criterion §17.2 ("better storage
efficiency than Git LFS") met with a wide margin, plus a transfer win LFS
cannot express at all.

**Honest negatives, measured:**

1. **Full rewrites**: no shared chunks → parity with LFS single-copy, 2× on
   a directory remote. CDC cannot help when every byte changes.
2. **Many small files with scattered small edits** (default 64k profile):
   chunk write-amplification eats most of the dedup (8.5 %); wire still
   −37 %, single-copy storage −7 %, but the directory remote's 2× makes
   total remote bytes worse than LFS. Tuning `--profile fastcdc-16k` fits
   this shape better.
3. **Per-object push overhead on many-object pushes**: v1 of many-files
   (250 objects) took 19.9 s vs 3.6 s vanilla — the agent re-exports the
   static tree after *every* upload (correctness: an acked object must be
   fetchable), which is O(assets) per upload today. Batching the export per
   push session (export once before the final `complete` of a batch, or a
   per-asset export API in cavs-store) is the obvious next optimization.
   Same root cause: 325 packfiles (one per upload) — a store `gc`/repack
   pass would consolidate.
4. **Directory-remote 2× storage** is the price of a zero-server,
   CDN-syncable remote; the CAVS Cloud backend of the spec stores one copy
   (the "single-copy" column).

**Plain git as a baseline**: `gc --aggressive` delta compression is
extremely effective on storage (best in 2 of 4 scenarios) — but it is
server-side CPU the host pays on every repack, clones always carry full
history, and GitHub rejects >100 MB files, which is why LFS-shaped systems
exist. CAVS gets within 1.1–6× of git's storage while keeping O(changed
bytes) transfers and no server-side compute at all (static remote).

## Spec §17 success criteria

| criterion | result |
|---|---|
| Git LFS compatible | ✅ real git-lfs 3.7.1: 44 pushes/pulls/clones across 12 combos |
| Better storage efficiency than Git LFS | ✅ on versioned large binaries (−54…−88 %); ❌ small-file trees on dir remotes (see findings 2/4) |
| Correct reconstruction | ✅ sha256-verified at every version in every system (44/44) |
| Benchmark reports | ✅ this report + `results/results.csv` + per-store stats |
