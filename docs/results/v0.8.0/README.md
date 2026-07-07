# v0.8.0 benchmark results — raw outputs & reproduction

Raw outputs of the v0.8.0 benchmark suite (A–H). Every table in
[ROUTE_BENCHMARKS.md](../../ROUTE_BENCHMARKS.md) and
[BENCHMARKS.md](../../BENCHMARKS.md) derives from the files in this
directory; the exact commands to regenerate them are below.

## Environment

| | |
|---|---|
| CPU | Apple M3 Pro (12 cores: 6P + 6E) |
| RAM | 36 GiB |
| OS | macOS 26.5.1 (Darwin 25.5.0) |
| Filesystem | APFS (internal NVMe) |
| rustc | 1.96.1 (release build, `lto = "thin"`, `codegen-units = 1`) |
| butler | v15.28.0 (ref `012ea1af65dd`, darwin-amd64 build via Rosetta) |
| bsdiff / bspatch | 4.3 (Homebrew) |
| xdelta3 | 3.2.0 |
| zstd (library via crate) | 1.5.x; CLI 1.5.7 used for the blob dataset |
| brotli | 1.2.0 |
| Date | 2026-07-07 |

Peak RSS figures come from `/usr/bin/time -l` (macOS reports bytes).
CAVS apply times and RSS are measured by running the real release
binaries as subprocesses under the same `/usr/bin/time` wrapper the
external tools get — not in-process timers. Every route's output was
verified byte-identical before its size was reported. Wall-clock times
on other machines will differ; ratios have been stable across runs.

## Reproduction — exact commands

All commands from the repository root after
`cargo build --release -p cavs-cli`, with `CAVS=target/release/cavs`,
`BUTLER=<path to butler binary>`, and `$WORK` any scratch directory.

### Datasets

```sh
# A, F, G, H: synthetic directory pair (deterministic, seed 5)
$CAVS bench gen-dir --out $WORK/ds-dir --size 128MiB --seed 5

# B: artifact variants (v1.bin, v2-shifted.bin, ..., seed 5)
$CAVS bench gen --out $WORK/ds-art --size 128MiB --seed 5

# C: the same directory builds shipped as single compressed blobs
(cd $WORK/ds-dir/Build_v1 && tar cf - . | zstd -3 -q -o $WORK/ds-blob/game_v1.tar.zst)
(cd $WORK/ds-dir/Build_v2 && tar cf - . | zstd -3 -q -o $WORK/ds-blob/game_v2.tar.zst)

# D (all-pairs / hot-pair storage): 10 sequential versions, ~3% drift
python3 - <<'EOF'
import random, os
random.seed(5)
BLOCK, NBLOCKS = 64*1024, 512           # 32 MiB per version
blocks = [random.randbytes(BLOCK) for _ in range(NBLOCKS)]
for v in range(1, 11):
    if v > 1:
        for _ in range(15):             # ~3% of blocks per release
            blocks[random.randrange(NBLOCKS)] = random.randbytes(BLOCK)
    with open(f'{os.environ["WORK"]}/ds-stream/v{v}.bin','wb') as f:
        for b in blocks: f.write(b)
EOF

# E: 256 MiB artifact pair, ~3% block drift
python3 - <<'EOF'
import random, os
random.seed(7)
BLOCK, NBLOCKS = 64*1024, 4096          # 256 MiB
blocks = [random.randbytes(BLOCK) for _ in range(NBLOCKS)]
open(f'{os.environ["WORK"]}/ds-big/v1.bin','wb').write(b''.join(blocks))
for _ in range(123):
    blocks[random.randrange(NBLOCKS)] = random.randbytes(BLOCK)
open(f'{os.environ["WORK"]}/ds-big/v2.bin','wb').write(b''.join(blocks))
EOF
```

### A — typical directory release

```sh
$CAVS bench full-pipeline --old $WORK/ds-dir/Build_v1 --new $WORK/ds-dir/Build_v2 \
  --butler-bin $BUTLER --include-pairwise --out results/A-dir
```

→ [`A-directory/summary.json`](A-directory/summary.json) ·
[`summary.md`](A-directory/summary.md) · butler raw JSON lines in
[`A-directory/butler-raw/`](A-directory/butler-raw/).

### B — shifted artifact

```sh
$CAVS bench full-pipeline --old $WORK/ds-art/v1.bin --new $WORK/ds-art/v2-shifted.bin \
  --butler-bin $BUTLER --include-pairwise --out results/B-shifted
```

→ [`B-shifted/`](B-shifted/).

### C — compressed blob

```sh
$CAVS bench full-pipeline --old $WORK/ds-blob/game_v1.tar.zst --new $WORK/ds-blob/game_v2.tar.zst \
  --butler-bin $BUTLER --include-pairwise --out results/C-blob
```

→ [`C-compressed-blob/`](C-compressed-blob/).

### D — many-version storage

```sh
# Store-once model (writes version-stream.{md,json}):
$CAVS bench version-stream --out results/D-stream --size 32MiB --versions 10 --seed 5

# All-pairs baseline: 45 bsdiff patches over the ds-stream versions
for i in $(seq 1 9); do for j in $(seq $((i+1)) 10); do
  bsdiff $WORK/ds-stream/v$i.bin $WORK/ds-stream/v$j.bin $WORK/pairs/p_${i}_${j}.bsdiff
done; done
# measured total: 151,233,162 bytes (144.23 MiB) in 45 patches

# Hot pairs per policy (previous, top-installed from a share map):
$CAVS patch-policy --versions v1,v2,v3,v4,v5,v6,v7,v8,v9,v10 --distribution shares.json
$CAVS optimize-patch --old $WORK/ds-stream/v9.bin --new $WORK/ds-stream/v10.bin --algo auto --compression auto -o v9_to_v10.cavspatch
$CAVS optimize-patch --old $WORK/ds-stream/v8.bin --new $WORK/ds-stream/v10.bin --algo auto --compression auto -o v8_to_v10.cavspatch
$CAVS optimize-patch --old $WORK/ds-stream/v7.bin --new $WORK/ds-stream/v10.bin --algo auto --compression auto -o v7_to_v10.cavspatch
# measured: 983,459 + 1,966,629 + 2,622,110 bytes = 5.31 MiB (all plan-ops)
```

→ [`D-version-stream/`](D-version-stream/). CAVS store 30.60 MiB +
hot pairs 5.31 MiB = 35.91 MiB vs 144.23 MiB all-pairs (−75%).

### E — low-memory apply

```sh
$CAVS optimize-patch --old $WORK/ds-big/v1.bin --new $WORK/ds-big/v2.bin \
  --algo bsdiff --compression zstd-19 -o bsdiff.cavspatch
# measured: 8,041,002 bytes (7.67 MiB), generation 123.9 s

/usr/bin/time -l $CAVS apply-patch --old $WORK/ds-big/v1.bin \
  --patch bsdiff.cavspatch --out rebuilt.bin
# measured: apply 1184 ms, maximum resident set size 541,999,104 B (517 MiB)

$CAVS apply-patch --old $WORK/ds-big/v1.bin --patch bsdiff.cavspatch \
  --out r2.bin --memory-budget 128MiB
# measured: refused — CAVS-E-MEMORY-BUDGET-EXCEEDED: estimated peak 551.67 MiB

$CAVS diff-plan $WORK/ds-big/v1.bin $WORK/ds-big/v2.bin -o update.cavsplan
# measured: 7,999,018 bytes (7.63 MiB)
/usr/bin/time -l $CAVS apply --old $WORK/ds-big/v1.bin --plan update.cavsplan --out r3.bin
# measured: apply 382 ms, maximum resident set size 28,065,792 B (26.8 MiB)

$CAVS route-plan --installed $WORK/ds-big/v1.bin --new $WORK/ds-big/v2.bin \
  --patch bsdiff.cavspatch --plan update.cavsplan --profile low-memory
# measured: cavspatch [excluded] needs ~551.67 MiB > 128 MiB; chosen: cavsplan
```

### F — interrupted apply / recovery

```sh
$CAVS test apply-recovery --old $WORK/ds-dir/Build_v1 --new $WORK/ds-dir/Build_v2 \
  --out results/F-recovery
```

→ [`F-apply-recovery/apply-recovery.json`](F-apply-recovery/apply-recovery.json).
10 SIGKILLed runs recovered; corrupt plan rejected untouched; corrupted
old install self-healed via deduplicated content (output verified);
garbage staging re-staged.

### G — mod preservation

```sh
cp -R $WORK/ds-dir/Build_v1 install/ && mkdir install/mods
echo "my mod" > install/mods/user_mod.pck && echo "user tweaked" > install/user_config.ini
touch -t 202001010000 install/assets/asset_00.dat
$CAVS diff-plan $WORK/ds-dir/Build_v1 $WORK/ds-dir/Build_v2 -o update.cavsplan
$CAVS apply --old install --plan update.cavsplan --inplace --delete-removed-files
# measured: 6 written, 38 no-op, 2 deleted; mod + config intact; mtime preserved
$CAVS signature export $WORK/ds-dir/Build_v2 --raw -o v2.cavssig
$CAVS verify-install install --signature v2.cavssig --allow-extra-files   # exit 0
```

### H — developer workflow timings

```sh
/usr/bin/time -h $CAVS signature export $WORK/ds-dir/Build_v1 --raw -o v1.cavssig   # 0.28 s
/usr/bin/time -h $CAVS preview $WORK/ds-dir/Build_v2 --against v1.cavssig \
  --changes-only --detect-compressed-blobs                                          # 0.35 s
/usr/bin/time -h $CAVS diff-plan --old-signature v1.cavssig \
  $WORK/ds-dir/Build_v1 $WORK/ds-dir/Build_v2 -o update.cavsplan                    # 0.42 s
/usr/bin/time -h $CAVS verify-install $WORK/ds-dir/Build_v2 --signature v2.cavssig  # 0.10 s
```

## Known tradeoffs

| Tradeoff | Detail | Mitigation |
|---|---|---|
| Sidecar generation is slow under `--algo auto` | 29–44 s on the 128 MiB cases: every applicable candidate (bsdiff, xdelta3, plan, full) is actually generated and measured per file | Publisher-side, once per hot pair; force one algorithm (`--algo xdelta3`) when speed matters more than the last percent |
| Sidecars serve exactly one pair | A `.cavspatch` is useless for any other old version | Hot-pair policy caps the count; every other jump falls back to store routes |
| bsdiff strategies are memory-hungry at apply | ~old + new + patch resident (517 MiB measured on a 256 MiB build) | Estimated up front and refusable via `--memory-budget`; the planner excludes it on `low-memory` profiles |
| Block routes degrade on compressed blobs | 21.9 MiB vs 2.5 MiB on the same content change | Detected (`preview --detect-compressed-blobs`); the strategy optimizer routes such files through a byte-level delta; publishing folders remains the real fix |
| butler default `diff` is faster than sidecar generation and its apply is marginally faster on case A | 331 ms vs 391 ms apply on A | The auto-route (plan) still ties its optimized patch on bytes; CAVS trades a few ms of apply for 4× less memory |
| `route-plan` bootstrap/chunk figures can be estimates | Exact only when real files are passed (`--plan`, `--patch`, `--bootstrap`) | Estimates are labeled (`~`) in output and JSON (`"exact": false`) |
| brotli sections need the external `brotli` binary at apply time | `--compression auto` may pick brotli-9 for a section | zstd-19 is always available in-process; force `--compression zstd-19` for fully self-contained applies |
| Synthetic datasets | Deterministic PRNG builds, not shipped games | Shapes (block drift, shifts, archives) chosen to match the failure modes seen on real builds; dataset generation is fully reproducible above |
| Wall-clock times are machine-specific | Apple M3 Pro figures | Ratios (bytes, RSS, relative times) have been stable across runs; raw JSON kept here for scrutiny |
