# CAVS vs Wharf (itch.io) — measured comparison

Wharf is itch.io's incremental upload/download protocol: an rsync-style
diff over fixed 64 KiB blocks (weak rolling hash + strong hash), patch
files made of `DATA`/`BLOCK_RANGE` operations, Brotli compression. It is a
**pairwise patcher**: every old→new pair gets its own patch, generated and
stored per pair. CAVS is a **version-stream delivery layer**: each release
is packaged once into content-addressed chunks, and any client — from any
older version, with any cache state — converges on the newest bytes.

v0.6.0 exists because Wharf's best ideas apply to CAVS without changing
that architecture: old-version range reuse, compact signatures, range
coalescing, preferred sources, no-op detection and staged applies are now
part of the client (see [HYBRID_RECONSTRUCTION.md](HYBRID_RECONSTRUCTION.md)).

## Methodology — read this first

`cavs bench wharf` measures a **Wharf-style model**, not the official
`butler` binary: fixed 64 KiB blocks, weak rolling hash prefilter,
strong-hash confirmation, `DATA`/`BLOCK_RANGE` planning with coalescing.
Two deliberate substitutions: BLAKE3-256 as the strong hash (Wharf uses
MD5) and zstd-1 as the patch transport compression (Wharf recommends
Brotli q1; on our payloads they compress within 0.1 % of each other — see
the compression table below). `xdelta3 -9` and `bsdiff` run as extra
baselines when present on PATH. Reproduce with:

```bash
cavs bench gen --out ds --size 128MiB --seed 5
cavs bench wharf --old ds/v1.bin --new ds/v2-small.bin --out results/wharf-small
```

## Patch size (128 MiB synthetic suite, seed 5)

| Pair | Full re-download (zstd-19) | Wharf-style patch | xdelta3 -9 | bsdiff | CAVS update (chunks) |
|---|---:|---:|---:|---:|---:|
| small change  | 64.05 MiB | 1.94 MiB | 1.94 MiB | 1.96 MiB | 6.06 MiB |
| medium change | 64.05 MiB | 8.83 MiB | 8.82 MiB | 8.90 MiB | 26.28 MiB |
| shifted (every byte moves) | 64.06 MiB | 4.04 KiB | 4.65 KiB | 4.67 KiB | 10.90 KiB |

Generation: wharf-style 234–525 ms, CAVS 243–253 ms.
Apply: wharf-style 136–160 ms, CAVS 58–62 ms. The Wharf-style signature
(exchanged once, reusable for any diff against that version) is 88 KiB.

**Honest reading: pairwise patches win per-pair bytes.** A diff that ships
only the dirty regions of dirty blocks, compressed as one stream, beats
shipping whole content-addressed chunks — on this suite by ~3× for
scattered small edits. That is inherent, not an implementation gap: chunk
granularity is what buys CAVS its operational properties. Where the byte
gap matters most (players updating over slow links, single title, adjacent
versions), Wharf's model is genuinely strong — it is why itch.io built it.

## What the per-pair number does not capture

| Property | Wharf / xdelta3 / bsdiff | CAVS |
|---|---|---|
| Packaging work per release | one patch **per old→new pair** (or a patch chain) | package **once**; all jumps served |
| v1→v5 direct jump | needs that exact patch, or applies 4 chained patches | same chunk fetch as any update |
| Storage across N versions | O(N²) patches (or O(N) chain + slow far jumps) | one deduplicated chunk store |
| CDN cacheability | per-pair patch files, per-audience | immutable content-addressed chunks/packs shared by every jump |
| Partial/corrupt local state | patch applies to a pristine old file or fails | cache verify/repair; corrupt ranges demote and re-fetch |
| Resume | restart patch download (tool-dependent) | resumable by design (chunks + HTTP ranges) |
| Byte-identical guarantee | final hash check (tool-dependent) | per-chunk BLAKE3 + final SHA-256, always |

And the v0.6.0 headline: with **no cache at all but the old version on
disk** — the situation a pairwise patcher assumes — CAVS now pays close to
patch-size bytes instead of a full re-download:

| Scenario (small change) | wire bytes |
|---|---:|
| CAVS v0.5, cold cache | 64.55 MiB |
| **CAVS v0.6, cold cache + previous install** | **6.24 MiB (−90.3 %)** |
| CAVS warm cache (v0.5 = v0.6) | 6.24 MiB |
| Wharf-style patch (pairwise, pre-generated) | 1.94 MiB |

## Compression: zstd vs Brotli

Wharf recommends Brotli q1 (transport) / q9 (storage). Measured on this
suite's payload (`cavs bench compression`, 32 MiB sample, brotli feature
enabled):

| algo | size | ratio | encode ms | decode ms |
|---|---:|---:|---:|---:|
| zstd-1  | 16.02 MiB | 0.501 | 6    | 4 |
| zstd-3  | 16.02 MiB | 0.501 | 14   | 1 |
| zstd-9  | 16.01 MiB | 0.500 | 19   | 2 |
| zstd-19 | 16.01 MiB | 0.500 | 1216 | 1 |
| brotli-1 | 16.02 MiB | 0.501 | 52  | 77 |
| brotli-9 | 16.01 MiB | 0.500 | 1113 | 77 |

Identical sizes within 0.1 %, zstd decodes ~40× faster here. **zstd-3 stays
the default**; Brotli remains available behind `--features brotli-bench`
for payload classes where it might win (rerun on your own builds).

## Framing

Wharf inspired v0.6.0 to add hybrid reconstruction. CAVS now combines
content-addressed storage with old-version range reuse — it does not claim
to beat a pairwise patcher at its own single-pair game, and the numbers
above say so explicitly.
