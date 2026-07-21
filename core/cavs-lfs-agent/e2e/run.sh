#!/usr/bin/env bash
# End-to-end test of cavs-lfs-agent against real git + git-lfs.
#
# Creates a bare origin, pushes a 20 MiB binary through the agent
# (standalone custom transfer), clones fresh and verifies the round-trip,
# then pushes a small modification and asserts chunk-level dedup at the
# remote (pack growth ≪ file size). Verifies stdout discipline: the agent
# must emit protocol JSON only.
#
# Usage: e2e/run.sh [path-to-agent-binary]
#   AGENT=... e2e/run.sh          # explicit binary
#   e2e/run.sh                    # builds with cargo if needed
set -euo pipefail

if ! command -v git-lfs >/dev/null 2>&1; then
  echo "SKIP: git-lfs is not installed"
  exit 0
fi

HERE=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$HERE/../../.." && pwd)

AGENT=${AGENT:-${1:-}}
if [ -z "$AGENT" ]; then
  echo "[e2e] building cavs-lfs-agent…"
  (cd "$REPO_ROOT" && cargo build -q -p cavs-lfs-agent)
  AGENT="$REPO_ROOT/target/debug/cavs-lfs-agent"
fi
[ -x "$AGENT" ] || { echo "FAIL: agent binary not found at $AGENT"; exit 1; }
echo "[e2e] agent: $AGENT"

WORK=$(mktemp -d)
# On failure, surface the captured git/git-lfs/agent stderr before cleanup —
# otherwise CI shows a bare exit code.
cleanup() {
  st=$?
  if [ "$st" -ne 0 ]; then
    echo "--- e2e failed (exit $st); captured stderr follows:"
    for f in "$WORK"/*.err; do
      [ -s "$f" ] || continue
      echo "----- $f"; cat "$f"
    done
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT
echo "[e2e] workdir: $WORK"
git lfs version

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
pack_kb() { du -sk "$WORK/origin.git/cavs/chunks" 2>/dev/null | cut -f1 || echo 0; }

# Deterministic pseudo-random data (seeded), so runs are reproducible.
blob() { # blob <bytes> <seed> > out
  python3 -c "
import sys, random
n, seed = int(sys.argv[1]), int(sys.argv[2])
r = random.Random(seed)
sys.stdout.buffer.write(bytes(r.getrandbits(8) for _ in range(n)))
" "$1" "$2"
}

configure_lfs() { # configure_lfs <repo-dir>
  git -C "$1" config lfs.standalonetransferagent cavs
  git -C "$1" config lfs.customtransfer.cavs.path "$AGENT"
  git -C "$1" config lfs.customtransfer.cavs.concurrent false
}

echo "[e2e] 1. bare origin + working clone"
git init -q --bare "$WORK/origin.git"
git init -q "$WORK/repo"
git -C "$WORK/repo" remote add origin "$WORK/origin.git"
git -C "$WORK/repo" config user.email e2e@example.com
git -C "$WORK/repo" config user.name e2e
configure_lfs "$WORK/repo"
(cd "$WORK/repo" && git lfs install --local >/dev/null)
(cd "$WORK/repo" && git lfs track '*.bin' >/dev/null)

echo "[e2e] 2. commit + push a 20 MiB binary through the agent"
blob 20971520 42 > "$WORK/repo/big.bin"
SHA_V1=$(sha "$WORK/repo/big.bin")
git -C "$WORK/repo" add .
git -C "$WORK/repo" commit -qm v1
git -C "$WORK/repo" push -q origin main 2> "$WORK/push1.err"
DU1=$(pack_kb)
[ "$DU1" -gt 0 ] || { echo "FAIL: no packs at origin after push"; exit 1; }
echo "[e2e]    origin cavs store: ${DU1} KiB"

echo "[e2e] 3. fresh clone pulls through the agent (bare-repo auto cavs/)"
# Filter configs are passed explicitly so the test is self-contained even on
# machines that never ran `git lfs install` globally.
git clone -q \
  -c 'filter.lfs.smudge=git-lfs smudge -- %f' \
  -c 'filter.lfs.clean=git-lfs clean -- %f' \
  -c 'filter.lfs.process=git-lfs filter-process' \
  -c filter.lfs.required=true \
  -c lfs.standalonetransferagent=cavs \
  -c "lfs.customtransfer.cavs.path=$AGENT" \
  -c lfs.customtransfer.cavs.concurrent=false \
  "$WORK/origin.git" "$WORK/clone2" 2> "$WORK/clone.err"
configure_lfs "$WORK/clone2"                       # persist for later pulls
(cd "$WORK/clone2" && git lfs install --local >/dev/null)
GOT=$(sha "$WORK/clone2/big.bin")
[ "$GOT" = "$SHA_V1" ] || { echo "FAIL: round-trip sha mismatch: $GOT != $SHA_V1"; exit 1; }
echo "[e2e]    round-trip sha256 OK"

echo "[e2e] 4. modify 1 MiB + append 2 MiB, push, expect chunk-level dedup"
blob 1048576 1337 | dd of="$WORK/repo/big.bin" bs=1048576 seek=5 conv=notrunc 2>/dev/null
blob 2097152 7 >> "$WORK/repo/big.bin"
SHA_V2=$(sha "$WORK/repo/big.bin")
git -C "$WORK/repo" commit -qam v2
git -C "$WORK/repo" push -q origin main 2> "$WORK/push2.err"
DU2=$(pack_kb)
GROWTH=$((DU2 - DU1))
echo "[e2e]    packs: ${DU1} KiB -> ${DU2} KiB (+${GROWTH} KiB for ~3 MiB of changes in a 22 MiB file)"
# Allow generous slack over the ~3 MiB of changed data; far below 22 MiB.
[ "$GROWTH" -lt 8192 ] || { echo "FAIL: expected dedup, packs grew ${GROWTH} KiB"; exit 1; }

echo "[e2e] 5. pull v2 in the clone (delta download over warm cache)"
git -C "$WORK/clone2" pull -q 2> "$WORK/pull.err"
GOT2=$(sha "$WORK/clone2/big.bin")
[ "$GOT2" = "$SHA_V2" ] || { echo "FAIL: v2 sha mismatch: $GOT2 != $SHA_V2"; exit 1; }
echo "[e2e]    v2 sha256 OK"

echo "[e2e] 6. re-push same commit is a no-op"
git -C "$WORK/repo" push -q origin main
DU3=$(pack_kb)
[ "$DU3" -eq "$DU2" ] || { echo "FAIL: idempotent re-push grew packs ${DU2} -> ${DU3}"; exit 1; }

echo "[e2e] 7. stdout discipline: agent transcript is protocol JSON only"
# GIT_TRACE surfaces any junk the agent wrote to stdout as xfer errors.
if grep -qiE "unexpected|invalid|panic" "$WORK/push1.err" "$WORK/push2.err" "$WORK/clone.err" "$WORK/pull.err"; then
  echo "FAIL: git-lfs reported protocol trouble:"
  grep -iE "unexpected|invalid|panic" "$WORK"/*.err
  exit 1
fi

echo "PASS: cavs-lfs-agent e2e (round-trip, dedup, idempotent re-push)"
