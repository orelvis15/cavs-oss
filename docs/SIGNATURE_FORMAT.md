# CAVS Signature v1 — `.cavssig`

A `.cavssig` is a compact description of an old artifact or directory tree —
layout, sizes and per-block hashes — so a new version can be compared
against it **without the old content** (the same role a signature plays in
rsync-style delta patching). CAVS-native details: BLAKE3-256 is the only
identity/verification hash, and the weak 32-bit rolling hash is a prefilter
that is never trusted on its own.

For a 128 MiB build the signature is ~88 KiB (0.07 % of the source).
Encoding is deterministic: the same logical signature always produces the
same bytes, and the decoder only accepts the canonical form.

## CLI

```bash
cavs signature export game_v1.cavs -o game_v1.cavssig      # from a container
cavs signature export --raw game_v1.pck -o game_v1.cavssig # from a raw file
cavs signature export --raw ./Build_v1 -o build_v1.cavssig # from a directory
cavs signature inspect game_v1.cavssig           # --json for tooling
cavs signature ls build_v1.cavssig               # every entry (v0.7.0)
cavs signature verify game_v1.cavssig --against game_v1.pck

# Report reusable bytes at pack time, old content not required:
cavs pack --raw game_v2.pck --against-signature game_v1.cavssig -o game_v2.cavs
```

Signatures also drive the v0.7.0 offline toolkit: `cavs preview`
classifies a new build against one, `cavs diff-plan` diffs against one
(`--old-signature`), and `cavs verify-install` checks an install against
one ([OFFLINE_TOOLKIT.md](OFFLINE_TOOLKIT.md)).

Exporting from a `.cavs` streams the *reconstructed* data tracks through
the block hasher, so the signature describes what a client's previous
install actually looks like on disk.

## Wire layout

All multi-byte integers are strict LEB128 varints (truncation, overlong
forms and u64 overflow rejected) unless noted. Hashes are raw 32-byte
BLAKE3-256.

```text
[8]  magic  "CAVSSIG1"
u16  version = 1                      (LE, fixed width)
u8   kind                             (1 single-artifact, 2 directory-container)
var  created_at_unix_ms               (0 in deterministic exports — the default)
var  block_size                       (default 65536; bounds 1 KiB .. 64 MiB)
var  source_size
u8   has_source_blake3;  [32] if 1    (full content hash)
str  source_label                     (var len + UTF-8)
str  chunker_profile
var  entry_count                      (cap 2^24)
     entry_count × {
       var entry_id                   (unique, dense from 0)
       str path                       (relative; "" or name for artifacts)
       u8  kind                       (1 file, 2 directory, 3 symlink)
       var size
       u8  executable                 (0/1)
       str symlink_target             ("" = none)
     }
var  block_count                      (cap 2^28)
     block_count × {
       var entry_id                   (must reference a declared entry)
       var offset                     (contiguous from 0 per entry)
       var len                        (1 .. block_size)
       u32 weak32                     (LE; rolling-hash prefilter)
       [32] strong_blake3
     }
[32] merkle_root                      (over block strong hashes, in order)
[32] integrity trailer                (BLAKE3 of every preceding byte)
```

Decoder guarantees:

- The integrity trailer is checked before anything is parsed; one flipped
  bit anywhere rejects the file (`CAVS-E-SIGNATURE-CORRUPT`).
- Blocks of each file entry must be contiguous from offset 0 and cover the
  entry's size exactly — a decoded signature can never describe an
  out-of-range read of its source.
- Entry ids must be unique; counts are capped before allocation; the Merkle
  root must recompute from the block hashes.
- Decode∘encode is the identity on canonical bytes (fuzzed:
  `fuzz_signature_decode`).

## Weak hash

rsync-style Adler variant over a window `x_0..x_{l-1}` (all mod 2^16):

```text
a = Σ x_i          b = Σ (l − i)·x_i          weak32 = a | (b << 16)
```

Rolling one byte is O(1): `a' = a − out + in`, `b' = b − l·out + a'`.
A weak match is always confirmed with BLAKE3 before any byte is reused.

## Why explicit entry_id/offset per block?

A fixed-block scheme could infer block positions from a container layout and
a constant block size. CAVS stores them explicitly because signatures may
later mix fixed and content-defined profiles, explicit offsets make
source-range reuse trivial, and it removes ambiguity as the format evolves.
The cost is a few varint bytes per block — irrelevant at 0.07 % of source
size.
