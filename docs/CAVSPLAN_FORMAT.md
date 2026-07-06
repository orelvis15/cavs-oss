# `.cavsplan` — offline reconstruction plan format (v1)

A `.cavsplan` is a deterministic, self-verifying description of how to
turn an old build into a new one. It is produced by `cavs diff-plan`
(and internally by `cavs bench routes`) from the new build plus the old
build's `.cavssig` — the old bytes are not required to diff.

## Kinds

| Kind | Payload | Use |
|---|---|---|
| `analysis` | none | previews, reports, CI size gates |
| `portable` | inline data (zstd) | self-contained offline patch for `cavs apply` |

A third shape from the v0.7.0 design — server-assisted plans with URL and
packfile hints — is covered today by the store export's per-asset
`chunk-map.json` (`cavs store export --static-plans`).

## Wire layout

All multi-byte integers are strict LEB128 varints unless noted. Strings
are varint length + UTF-8 bytes.

```text
[8]  magic  "CAVSPLN1"
u16  version = 1                     (LE)
u8   kind                            (1 analysis, 2 portable)
u8   mode                            (1 artifact, 2 directory)
var  block_size                      (of the .cavssig used to diff)
str  old_label; var old_size
u8   has_old_blake3; [32] if set     (full BLAKE3 of the old content)
str  new_label; var new_size
var  old_entry_count
     × { var entry_id; str path; var size }          (COPY sources)
var  new_entry_count
     × { var entry_id; str path; u8 kind;            (1 file, 2 dir, 3 symlink)
         var size; u8 executable; str symlink_target ("" = none);
         u8 has_blake3; [32] if set }                (files always carry one)
var  deleted_count × { str path }                    (managed deletions)
var  op_count × ops                                  (see below)
var  blob_raw_len; var blob_comp_len; [blob]         (zstd; empty in analysis)
[32] BLAKE3 of every preceding byte                  (integrity trailer)
```

Ops (tag byte first):

```text
1 COPY   { var old_entry_id; var old_offset;
           var new_entry_id; var new_offset; var len }
2 INLINE { var new_entry_id; var new_offset; var len; var blob_offset }
```

## Invariants the decoder enforces

- integrity trailer first — a flipped bit anywhere is
  `CAVS-E-PLAN-CORRUPT` before any field is trusted;
- every path is relative, forward-slash, without `.`/`..`/`\`/`:`
  components (`CAVS-E-PATH-TRAVERSAL` otherwise);
- per file, ops tile `0..size` exactly — no gaps, no overlaps
  (`CAVS-E-PLAN-INVALID`);
- ops reference only declared entries; inline ops stay inside the blob;
- entry ids are unique; file entries always carry a content BLAKE3.

A decoded plan is therefore always internally consistent; `cavs apply`
re-verifies every output hash on top of that.

## Determinism

Encoding is canonical: the same old signature + new bytes always produce
the same plan bytes (timestamps are not recorded). Plans can be diffed,
cached, and content-addressed. `cavs file update.cavsplan` and
`cavs ls update.cavsplan` inspect them.

## Size behaviour (measured, 128 MiB builds)

The inline payload is compressed as one zstd-19 stream, so a plan is
usually *smaller* than the equivalent per-chunk wire transfer: 2.51 MiB
vs 5.42 MiB on the directory pair, 1.94 MiB vs 6.06 MiB on the artifact
pair, 4.21 KiB vs 10.9 KiB on the shifted artifact. The trade is
generation cost (zstd-19) paid once at release time.
