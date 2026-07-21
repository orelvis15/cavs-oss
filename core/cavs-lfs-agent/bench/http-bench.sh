#!/usr/bin/env bash
# HTTP download benchmark: cold clone through the LFS agent from an
# http:// CAVS remote (the CDN scenario). Datasets are pushed to a
# directory remote with AGENT_PUSH, then the export tree is served over
# HTTP (bench/range_server.py — python's http.server ignores Range) and
# cloned cold once per agent variant, counting wall time and the HTTP
# requests the server saw.
#
# Usage:  AGENT_A=/path/a AGENT_B=/path/b bench/http-bench.sh [scenarios…]
#         (defaults: scenarios "big-binary many-files"; AGENT_B optional —
#          omit to measure a single agent)
#         LATENCY_MS=25 emulates WAN: the server sleeps that long before
#         every response, so each round-trip costs what a real CDN would.
# Output: CSV on stdout: scenario,agent,metric,value
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
[ -n "${AGENT_A:-}" ] || { echo "AGENT_A=<agent binary> required" >&2; exit 1; }
AGENT_PUSH=${AGENT_PUSH:-$AGENT_A}
SCENARIOS=${*:-"big-binary many-files"}

WORK=$(mktemp -d)
SRV_PID=
trap '[ -n "$SRV_PID" ] && kill "$SRV_PID" 2>/dev/null; rm -rf "$WORK"' EXIT
echo "[http-bench] workdir: $WORK" >&2

python3 "$HERE/gen.py" "$WORK/data" >/dev/null

lfs_filter_cfg=(-c 'filter.lfs.smudge=git-lfs smudge -- %f'
                -c 'filter.lfs.clean=git-lfs clean -- %f'
                -c 'filter.lfs.process=git-lfs filter-process'
                -c filter.lfs.required=true)

now() { python3 -c 'import time; print(f"{time.time():.3f}")'; }

prepare_origin() { # <scenario> -> echoes origin path
  local SC=$1 ORIGIN="$WORK/$SC-origin.git" REPO="$WORK/$SC-repo"
  git init -q -b main --bare "$ORIGIN"
  git init -q -b main "$REPO"
  git -C "$REPO" config user.email b@x && git -C "$REPO" config user.name b
  (cd "$REPO" && git lfs install --local >/dev/null)
  git -C "$REPO" config lfs.standalonetransferagent cavs
  git -C "$REPO" config lfs.customtransfer.cavs.path "$AGENT_PUSH"
  git -C "$REPO" config lfs.customtransfer.cavs.concurrent false
  printf '*.bin filter=lfs diff=lfs merge=lfs -text\n*.pak filter=lfs diff=lfs merge=lfs -text\n' > "$REPO/.gitattributes"
  git -C "$REPO" remote add origin "$ORIGIN"
  local v
  for v in "$WORK/data/$SC"/v*; do
    find "$REPO" -mindepth 1 -maxdepth 1 ! -name .git ! -name .gitattributes -exec rm -rf {} +
    cp -R "$v/." "$REPO/"
    git -C "$REPO" add -A && git -C "$REPO" commit -qm "$(basename "$v")"
    # git-lfs prints "Uploading…" progress on stdout; keep stdout clean so
    # the caller can capture the origin path.
    git -C "$REPO" push -q origin main >/dev/null 2>&1
  done
  echo "$ORIGIN"
}

serve() { # <dir> <log> — sets SRV_PID and SRV_URL; requests land in <log>
  SRV_LOG=$2
  python3 -u "$HERE/range_server.py" "$1" > "$SRV_LOG.port" 2> "$SRV_LOG" &
  SRV_PID=$!
  until grep -q '^PORT' "$SRV_LOG.port" 2>/dev/null; do sleep 0.1; done
  SRV_URL="http://127.0.0.1:$(awk '{print $2}' "$SRV_LOG.port")"
}

clone_via() { # <label> <agent> <origin> <scenario>
  local LABEL=$1 AGENT=$2 ORIGIN=$3 SC=$4
  local DST="$WORK/$SC-clone-$LABEL" CACHE="$WORK/$SC-cache-$LABEL"
  local before after t0 t1
  before=$(grep -c '"GET' "$SRV_LOG" || true)
  t0=$(now)
  CAVS_LFS_REMOTE="$SRV_URL" CAVS_LFS_CACHE="$CACHE" \
    git clone -q "${lfs_filter_cfg[@]}" \
    -c lfs.standalonetransferagent=cavs \
    -c "lfs.customtransfer.cavs.path=$AGENT" \
    -c lfs.customtransfer.cavs.concurrent=false \
    "$ORIGIN" "$DST"
  t1=$(now)
  after=$(grep -c '"GET' "$SRV_LOG" || true)
  local LATEST; LATEST=$(ls -d "$WORK/data/$SC"/v* | sort -V | tail -1)
  if ! diff <(cd "$DST" && find . -type f ! -path './.git/*' ! -name .gitattributes -exec shasum -a 256 {} + | sort -k2) \
            <(cd "$LATEST" && find . -type f -exec shasum -a 256 {} + | sort -k2) >/dev/null; then
    echo "FAIL: $SC/$LABEL clone mismatch" >&2; exit 1
  fi
  python3 -c "print(f'$SC,$LABEL,time_s,{$t1-$t0:.2f}')"
  echo "$SC,$LABEL,http_requests,$((after - before))"
}

echo "scenario,agent,metric,value"
for SC in $SCENARIOS; do
  ORIGIN=$(prepare_origin "$SC")
  serve "$ORIGIN/cavs" "$WORK/$SC-server.log"
  clone_via agent-a "$AGENT_A" "$ORIGIN" "$SC"
  [ -n "${AGENT_B:-}" ] && clone_via agent-b "$AGENT_B" "$ORIGIN" "$SC"
  kill "$SRV_PID" 2>/dev/null || true; wait "$SRV_PID" 2>/dev/null || true; SRV_PID=
done
