#!/usr/bin/env bash
# LFS storage/transfer benchmark: plain git vs vanilla Git LFS vs
# Git LFS + cavs-lfs-agent, over deterministic versioned datasets.
#
# Measures, per scenario and system:
#   - remote storage after every pushed version (KiB, du)
#   - push wall time per version
#   - incremental update in a tracking clone: downloaded bytes + time
#   - cold clone at latest version: downloaded bytes + time
#   - sha256 correctness of every reconstructed file
#   - CAVS store stats (objects, unique chunks, dedup %)
#
# Usage: bench/run.sh [out-dir]        (default: ./bench-results)
# Env:   AGENT=<path>  KEEP=1 (keep workdir)  SCENARIOS="big-binary …"
set -euo pipefail

command -v git-lfs >/dev/null || { echo "git-lfs required"; exit 1; }

HERE=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$HERE/../../.." && pwd)
OUT=${1:-"$PWD/bench-results"}
mkdir -p "$OUT"
CSV="$OUT/results.csv"
echo "scenario,system,phase,version,metric,value" > "$CSV"

AGENT=${AGENT:-}
if [ -z "$AGENT" ]; then
  echo "[bench] building cavs-lfs-agent + cavs (release)…"
  (cd "$REPO_ROOT" && cargo build -q --release -p cavs-lfs-agent -p cavs-cli)
  AGENT="$REPO_ROOT/target/release/cavs-lfs-agent"
fi
CAVS_CLI="$REPO_ROOT/target/release/cavs"

WORK=$(mktemp -d)
[ "${KEEP:-0}" = "1" ] || trap 'rm -rf "$WORK"' EXIT
echo "[bench] workdir: $WORK"
echo "[bench] results: $CSV"

DATA="$WORK/data"
python3 "$HERE/gen.py" "$DATA"

now() { python3 -c 'import time; print(f"{time.time():.3f}")'; }
elapsed() { python3 -c "print(f'{$2 - $1:.2f}')"; }
# Sum of file sizes in KiB. NOT du: on APFS, freshly written packfiles keep
# over-allocated blocks for a while, so du overstates (and hides growth).
if [ "$(uname)" = Darwin ]; then STAT_SIZE=(-f %z); else STAT_SIZE=(-c %s); fi
kb() {
  [ -e "$1" ] || { echo 0; return; }
  find "$1" -type f -exec stat "${STAT_SIZE[@]}" {} + 2>/dev/null \
    | awk '{s+=$1} END{printf "%d\n", s/1024}'
}
emit() { echo "$1,$2,$3,$4,$5,$6" >> "$CSV"; }   # scenario system phase version metric value

# sha256 manifest of a tree (relative paths), for correctness checks.
tree_sha() { (cd "$1" && find . -type f ! -path './.git/*' -exec shasum -a 256 {} + | sort -k2); }

versions_of() { ls -d "$DATA/$1"/v* | wc -l | tr -d ' '; }

# Copy dataset version into a working tree (leaves .git alone).
sync_version() { # sync_version <scenario> <v> <repo>
  find "$3" -mindepth 1 -maxdepth 1 ! -name .git ! -name .gitattributes -exec rm -rf {} +
  cp -R "$DATA/$1/v$2/." "$3/"
}

lfs_filter_cfg=(-c 'filter.lfs.smudge=git-lfs smudge -- %f'
                -c 'filter.lfs.clean=git-lfs clean -- %f'
                -c 'filter.lfs.process=git-lfs filter-process'
                -c filter.lfs.required=true)
cavs_cfg=(-c lfs.standalonetransferagent=cavs
          -c "lfs.customtransfer.cavs.path=$AGENT"
          -c lfs.customtransfer.cavs.concurrent=false)

# ---------------------------------------------------------------------------
# run_system <scenario> <system>   where system ∈ git | lfs | cavs
# ---------------------------------------------------------------------------
run_system() {
  local SC=$1 SYS=$2
  local ROOT="$WORK/$SC-$SYS"
  local ORIGIN="$ROOT/origin.git" REPO="$ROOT/repo" TRACK="$ROOT/track" COLD="$ROOT/cold"
  local NV; NV=$(versions_of "$SC")
  mkdir -p "$ROOT"
  echo "[bench] === $SC / $SYS ($NV versions) ==="

  git init -q --bare "$ORIGIN"
  git init -q "$REPO"
  git -C "$REPO" config user.email bench@example.com
  git -C "$REPO" config user.name bench

  local REMOTE_URL="$ORIGIN" DL_DIR_TRACK= DL_DIR_COLD= REMOTE_MEASURE="$ORIGIN"
  case $SYS in
    git) ;;
    lfs)
      REMOTE_URL="file://$ORIGIN"
      (cd "$REPO" && git lfs install --local >/dev/null)
      printf '*.bin filter=lfs diff=lfs merge=lfs -text\n*.pak filter=lfs diff=lfs merge=lfs -text\n' \
        > "$REPO/.gitattributes"
      ;;
    cavs)
      (cd "$REPO" && git lfs install --local >/dev/null)
      git -C "$REPO" config lfs.standalonetransferagent cavs
      git -C "$REPO" config lfs.customtransfer.cavs.path "$AGENT"
      git -C "$REPO" config lfs.customtransfer.cavs.concurrent false
      printf '*.bin filter=lfs diff=lfs merge=lfs -text\n*.pak filter=lfs diff=lfs merge=lfs -text\n' \
        > "$REPO/.gitattributes"
      ;;
  esac
  git -C "$REPO" remote add origin "$REMOTE_URL"

  # --- push every version, measuring remote growth + time ---------------
  local v prev_kb=0 t0 t1
  for v in $(seq 1 "$NV"); do
    sync_version "$SC" "$v" "$REPO"
    git -C "$REPO" add -A
    git -C "$REPO" commit -qm "v$v"
    t0=$(now)
    git -C "$REPO" push -q origin main 2> "$ROOT/push$v.err"
    t1=$(now)
    # plain git: let the remote repack like a real host would
    [ "$SYS" = git ] && git -C "$ORIGIN" gc -q --aggressive --prune=now 2>/dev/null
    local total_kb; total_kb=$(kb "$REMOTE_MEASURE")
    emit "$SC" "$SYS" push "$v" time_s "$(elapsed "$t0" "$t1")"
    emit "$SC" "$SYS" push "$v" remote_kb "$total_kb"
    emit "$SC" "$SYS" push "$v" remote_growth_kb "$((total_kb - prev_kb))"
    prev_kb=$total_kb

    # --- incremental update in the tracking clone ----------------------
    if [ "$v" = 1 ]; then
      t0=$(now)
      case $SYS in
        git)  git clone -q "$REMOTE_URL" "$TRACK" ;;
        lfs)  git clone -q "${lfs_filter_cfg[@]}" "$REMOTE_URL" "$TRACK" ;;
        cavs) git clone -q "${lfs_filter_cfg[@]}" "${cavs_cfg[@]}" "$REMOTE_URL" "$TRACK"
              git -C "$TRACK" config lfs.standalonetransferagent cavs
              git -C "$TRACK" config lfs.customtransfer.cavs.path "$AGENT"
              git -C "$TRACK" config lfs.customtransfer.cavs.concurrent false
              (cd "$TRACK" && git lfs install --local >/dev/null) ;;
      esac
      t1=$(now)
      case $SYS in
        git)  DL_DIR_TRACK="$TRACK/.git" ;;
        lfs)  DL_DIR_TRACK="$TRACK/.git/lfs/objects" ;;
        cavs) DL_DIR_TRACK="$TRACK/.git/lfs/cavs/cache" ;;
      esac
    else
      t0=$(now)
      git -C "$TRACK" pull -q 2> "$ROOT/pull$v.err"
      t1=$(now)
    fi
    local dl_kb; dl_kb=$(kb "$DL_DIR_TRACK")
    emit "$SC" "$SYS" update "$v" time_s "$(elapsed "$t0" "$t1")"
    emit "$SC" "$SYS" update "$v" dl_total_kb "$dl_kb"
    # correctness at every step
    if ! diff <(tree_sha "$TRACK" | grep -v .gitattributes) \
              <(tree_sha "$DATA/$SC/v$v") >/dev/null; then
      echo "FAIL: $SC/$SYS v$v tracking clone content mismatch"; exit 1
    fi
  done

  # --- cold clone at latest ---------------------------------------------
  t0=$(now)
  case $SYS in
    git)  git clone -q "$REMOTE_URL" "$COLD" ;;
    lfs)  git clone -q "${lfs_filter_cfg[@]}" "$REMOTE_URL" "$COLD" ;;
    cavs) git clone -q "${lfs_filter_cfg[@]}" "${cavs_cfg[@]}" "$REMOTE_URL" "$COLD" ;;
  esac
  t1=$(now)
  case $SYS in
    git)  DL_DIR_COLD="$COLD/.git" ;;
    lfs)  DL_DIR_COLD="$COLD/.git/lfs/objects" ;;
    cavs) DL_DIR_COLD="$COLD/.git/lfs/cavs/cache" ;;
  esac
  emit "$SC" "$SYS" clone_cold "$NV" time_s "$(elapsed "$t0" "$t1")"
  emit "$SC" "$SYS" clone_cold "$NV" dl_total_kb "$(kb "$DL_DIR_COLD")"
  if ! diff <(tree_sha "$COLD" | grep -v .gitattributes) \
            <(tree_sha "$DATA/$SC/v$NV") >/dev/null; then
    echo "FAIL: $SC/$SYS cold clone content mismatch"; exit 1
  fi
  emit "$SC" "$SYS" verify "$NV" sha256_ok 1

  # --- system-specific storage breakdown ---------------------------------
  case $SYS in
    cavs)
      emit "$SC" "$SYS" storage "$NV" store_kb "$(kb "$ORIGIN/cavs/.store")"
      emit "$SC" "$SYS" storage "$NV" export_kb "$(( $(kb "$ORIGIN/cavs") - $(kb "$ORIGIN/cavs/.store") ))"
      "$CAVS_CLI" store "$ORIGIN/cavs/.store" stat > "$OUT/$SC-store-stat.txt" 2>&1 || true
      # wire bytes per transfer, if the agent's stderr surfaced through git-lfs
      grep -h '\[lfs-agent\] download' "$ROOT"/pull*.err "$ROOT"/clone*.err 2>/dev/null \
        > "$OUT/$SC-wire.txt" || true
      ;;
    lfs)  emit "$SC" "$SYS" storage "$NV" lfs_objects_kb "$(kb "$ORIGIN/lfs")" ;;
    git)  emit "$SC" "$SYS" storage "$NV" git_kb "$(kb "$ORIGIN")" ;;
  esac

  # logical size of the latest version, once per scenario/system
  emit "$SC" "$SYS" logical "$NV" latest_version_kb "$(kb "$DATA/$SC/v$NV")"
  rm -rf "$ROOT"
}

SCENARIOS=${SCENARIOS:-"big-binary compressible many-files full-rewrite"}
for SC in $SCENARIOS; do
  for SYS in git lfs cavs; do
    run_system "$SC" "$SYS"
  done
done

echo
echo "[bench] done — raw metrics in $CSV"
