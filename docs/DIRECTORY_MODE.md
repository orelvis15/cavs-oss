# Directory / container mode (stable since v0.7.0)

Directory mode packages a build *folder* — the shape engines like Godot,
Unity and Unreal export, and the shape stores recommend uploading —
instead of a single archive. Each file becomes a deduplicated data track;
directories, symlinks and executable bits travel as metadata.

```bash
cavs pack-dir ./Build_v1 -o build_v1.cavs
cavs pack-dir ./Build_v2 -o build_v2.cavs --ignore '*.pdb' --ignore 'logs/'
```

## Why folders beat compressed archives for updates

Compression cascades: a one-line source change can rewrite most of a
compressed archive's bytes, so block-level patchers find nothing to
reuse. Measured on the same 126 MiB build and the same content change:

| Shape | CAVS update | butler offline |
|---|---:|---:|
| directory build | **2.51 MiB** | 2.52 MiB |
| same build as one zstd blob | 21.92 MiB | 21.92 MiB |

`cavs preview` warns when a large modified file looks compressed or
high-entropy for exactly this reason.

## Ignore rules

`--ignore <glob>` (repeatable) merges with a `.cavsignore` file at the
tree root (gitignore-lite):

```gitignore
# build junk
*.pdb
*.dSYM
logs/
temp/
```

- `*` and `?` match within one path segment, `**` crosses segments;
- a trailing `/` ignores a directory and everything under it;
- a pattern without `/` matches the basename at any depth; with `/` it
  anchors at the tree root;
- no negation — rules only exclude. `.cavsignore` itself is never packed.

## Path rules

Entries travel as UTF-8 forward-slash relative paths. Absolute paths,
`..` traversal, backslashes and drive-style colons are rejected at pack
time and again at decode time (`CAVS-E-PATH-TRAVERSAL`) — a hostile
container cannot write outside its root.

Determinism: the tree is walked in sorted order, so the same directory
always packs to the same logical container.

## Applying directory updates

Both the online client (`cavs-client fetch`) and the offline
`cavs apply` use the same staged model:

1. reconstruct changed files into `.cavs-staging/`;
2. verify every hash — nothing is committed on any mismatch;
3. journal intent (`.cavs-journal.json`);
4. commit with per-file renames; create dirs/symlinks; apply exec bits;
5. clean up. Re-running finishes an interrupted apply.

No-op and mod rules (defaults):

- unchanged files are never rewritten (timestamps survive);
- unknown extra files (mods, saves) are preserved;
- files the plan marks as removed are deleted only with
  `--delete-removed-files` (client: `--prune`).

## Platform notes

- Unix permissions are reduced to one executable bit; best-effort on
  Windows.
- Symlinks are recorded and recreated on Unix; on other platforms they
  are skipped with a `CAVS-E-UNSUPPORTED-SYMLINK` warning.
- Hardlinks are not detected (dedup makes the cost negligible).
