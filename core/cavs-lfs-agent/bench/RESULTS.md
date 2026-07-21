# benchmark-v1 — plain git vs Git LFS vs Git LFS + CAVS

**Frozen benchmark-v1** (git tag `benchmark-v1`). Measured 2026-07-21 on
macOS (Apple Silicon, APFS, local filesystem remotes), git 2.50.1, git-lfs
3.7.1, release builds, agent with size-tiered `--profile auto` (default).
Reproduce with `bench/run.sh` — datasets are deterministic (seeded). Raw
data: [results/benchmark-v1/](results/benchmark-v1/) (main run) and
[results/profile-sweep/](results/profile-sweep/) (chunking sweep).

These are the measurements the CAVS Cloud MVP spec asks for (§17):
Git LFS compatibility, storage efficiency vs Git LFS, reconstruction
correctness, and benchmark reports with the dashboard metrics (logical vs
physical storage, dedup %, objects, unique chunks).

## Systems

| | remote | what the remote stores |
|---|---|---|
| **git** | bare repo, `gc --aggressive` after every push | packfiles, binary deltas |
| **lfs** | vanilla Git LFS, `file://` standalone transfer | every object version, whole |
| **cavs** | `cavs-lfs-agent`, directory remote | shared chunk store **+ static export** (2 copies by design; a cloud backend holds 1 — both reported) |

Protocol per scenario: push v1…vN; after every push a tracking clone pulls
and its whole tree is sha256-verified; a cold clone and (CAVS) a warm clone
at vN are verified too. **14/14 verification gates passed — every byte
reconstructed identically in every system at every step.**

## Scenarios (deterministic, `bench/gen.py`)

| | shape | versions | change per version |
|---|---|---|---|
| big-binary | one 100→104 MiB incompressible asset | 5 | ~1.25 MiB edited + 1 MiB appended |
| compressible | one 64 MiB semi-structured pack (~2× zstd) | 4 | ~2 % of blocks replaced |
| many-files | 250 files, log-normal sizes, 86.5 MiB | 4 | 10 % of files partially edited |
| full-rewrite | one 48 MiB blob | 2 | 100 % rewritten (worst case) |
| cross-repo | two **unrelated** repos, similar 100 MiB content | 1+1 | repo-b ≈ repo-a + ~2 MiB |

## Storage at the remote (KiB, after last version)

| scenario | logical (latest) | git | vanilla LFS | CAVS total | **CAVS single-copy**¹ | CAVS vs LFS (single) |
|---|---:|---:|---:|---:|---:|---:|
| big-binary | 106 496 | 111 736 | 522 267 | 258 621 | **123 440** | **−76 %** |
| compressible | 65 536 | 20 128 | 262 171 | 56 918² | **26 339** | **−90 %** |
| many-files | 88 527 | 96 902 | 114 392 | 211 501 | 104 901 | −8 % |
| full-rewrite | 49 152 | 98 362 | 98 330 | 203 986 | 101 548 | +3 % |

¹ chunk store only — what a CAVS Cloud backend would bill. ² with
`auto`→16k + zstd, even the **doubled** directory remote (56.9 MB) is
smaller than one logical copy (65.5 MB) and 4.6× smaller than LFS.

CAVS store dedup (the spec's dashboard metrics, from `store stat`):

| scenario | objects | logical → stored | dedup |
|---|---:|---|---:|
| big-binary | 5 | 510.0 → ~119 MiB | **77 %** |
| compressible | 4 | 72.3 → ~22 MiB | **69 %** |
| many-files | 325 | 111.6 → ~99 MiB | 12 % |
| full-rewrite | 2 | 96.0 → 96.0 MiB | 0 % |

## Transfer

**Per-version update download** (tracking clone; CAVS = chunk-cache growth,
an upper bound on the zstd-compressed wire):

| scenario | git³ | vanilla LFS | **CAVS** | CAVS vs LFS |
|---|---:|---:|---:|---:|
| big-binary | 104 993 | 104 960 | **3 032** | **−97 %** |
| compressible | 18 997 | 65 536 | **4 603** | **−93 %** |
| many-files | 8 602 | 8 593 | **4 138** | **−52 %** |
| full-rewrite | 49 168 | 49 152 | 49 152 | 0 % |

³ plain-git update wire is transport-dependent (local path sent full packs
here; a smart-HTTP server would send thin deltas) — its storage column is
the honest git number.

**Warm clone (CAVS only)**: a second consumer sharing the populated chunk
cache downloaded **0 new KiB in every scenario** — vanilla LFS has no
cross-clone cache at all.

**Cross-repo dedup**: repo-b (unrelated git history, similar content)
pushed to the same remote:

| | repo-a push | repo-b push |
|---|---:|---:|
| vanilla LFS | 102 426 | 103 424 |
| **CAVS** | 212 487 (2-copy) | **11 294 (−89 %)** |

**Push wall time, all versions** (local disk): CAVS fastest on big-binary
(12.3 s vs LFS 14.1 s), competitive elsewhere (many-files 11.6 s vs LFS
10.1 s — was 42 s before the per-asset export batching; plain git is
fastest on small trees but pays server-side `gc --aggressive` CPU that no
one bills here).

## Chunking profile sweep → `auto`

Full data in [results/profile-sweep/](results/profile-sweep/). Single-copy
storage / avg update download per version (KiB):

| profile | big-binary (104 MiB) | compressible (64 MiB) | many-files (~360 KiB avg) |
|---|---|---|---|
| fastcdc-16k | 123 440 / **3 032** | **26 339 / 4 603** | **104 901 / 4 138** |
| fastcdc-64k | **119 268** / 3 722 | 32 753 / 16 069 | 105 874 / 5 355 |
| fastcdc-256k | 130 741 / 6 957 | 52 908 / 40 384 | 110 190 / 6 979 |
| fixed-1m | 149 672 / 11 776 | 58 479 / 47 104 | 114 423 / — |

Small chunks win everywhere on update download; 64k only edges out 16k on
raw storage of large incompressible blobs (+3.5 % at 16k, mostly metadata).
Larger chunks lose across the board. Hence the shipped `--profile auto`:
**<128 MiB → fastcdc-16k, <512 MiB → fastcdc-64k, else fastcdc-128k** — a
pure function of size so chunk boundaries (and cross-version dedup) stay
stable as a file evolves.

## Storage breakdown (CAVS, big-binary at 16k)

| component | KiB | share |
|---|---:|---:|
| chunk data (packs, zstd) | 114 529 | 88.5 % |
| chunk-map.json (export) | 9 926 | 3.8 % |
| store metadata (index + records) | 8 573 | 3.3 % |
| manifest.json (export) | 5 977 | 2.3 % |
| record.json (export) | 4 382 | 1.7 % |
| pack indexes | 337 | 0.1 % |

(Single copy; the directory remote duplicates the pack data into the
export.) At 16 k the pretty-printed JSON metadata is ~11 % — an obvious
next optimization (compact/binary manifests, or gzip at the CDN edge).

## Xet (Hugging Face)

`git-xet` (0.2.1, huggingface/xet-core) is the closest architectural
sibling — also a Git LFS custom transfer agent doing CDC + dedup. It could
**not** be benchmarked apples-to-apples: git-xet activates only when the
LFS **Batch API server** picks the `xet` agent in negotiation, i.e. it
requires a Xet-protocol CAS backend (in practice Hugging Face Hub's hosted
service; xet-core ships no self-hostable CAS server). Verified empirically:
with git-xet registered, a push to a `file://` remote stored the object
whole — xet never engaged. A benchmark against HF Hub would measure their
CDN and network, not the algorithms. Architecturally: both use CDC (~64 KiB
target for Xet; CAVS `auto` tiers 16k/64k/128k), BLAKE3-family hashing and
pack aggregation (Xet 64 MiB xorbs; CAVS 128 MiB packs); CAVS adds the
static/serverless export (any dumb HTTP host works) while Xet adds a
hosted global-dedupe service.

## Interruption & recovery (committed as tests, not one-off runs)

`tests/agent_roundtrip.rs`: the agent killed mid-upload (twice) leaves the
store uncorrupted — stray `.part` packs are cleaned on next open, the
flock dies with the process, and a re-push repairs publish + export;
killed mid-download, the partial chunk cache is reused and the retry
verifies byte-identical. Plus a 5-object single-session batch test
(one lock + one store open per push session).

## Spec §17 success criteria

| criterion | result |
|---|---|
| Git LFS compatible | ✅ real git-lfs 3.7.1 across all scenarios, incl. multi-object pushes |
| Better storage efficiency than Git LFS | ✅ −76…−90 % (single-copy) on versioned binaries; parity on the no-dedup worst case; −8 % on small-file trees |
| Correct reconstruction | ✅ 14/14 sha256 gates, incl. after crashes |
| Benchmark reports | ✅ this report + frozen raw CSVs + store stats |

## Known limits (measured, not hidden)

1. Directory remotes hold 2× the chunk data (store + CDN-syncable export);
   a cloud backend holds 1×.
2. Full rewrites cannot dedup — parity with LFS single-copy.
3. JSON metadata ≈ 11 % at 16k on big assets (compact encoding pending).
4. Small-file trees: modest wins (−8 % storage, −52 % download) — CDC
   granularity vs file size; still never worse on transfer.
