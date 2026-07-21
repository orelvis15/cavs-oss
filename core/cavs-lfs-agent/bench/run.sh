#!/usr/bin/env bash
# LFS storage/transfer benchmark: plain git vs vanilla Git LFS vs
# Git LFS + cavs-lfs-agent, over deterministic versioned datasets.
#
# Measures, per scenario and system:
#   - remote storage after every pushed version (KiB, summed file bytes)
#   - push wall time per version
#   - incremental update in a tracking clone: downloaded bytes + time
#   - cold clone at latest version: downloaded bytes + time
#   - warm clone (CAVS only): shared chunk cache, expected ~0 new bytes
#   - sha256 correctness of every reconstructed file
#   - CAVS storage breakdown: pack data / indexes / manifests / chunk-maps
#     / records / store metadata, store copy vs export copy
#   - CAVS store stats (objects, unique chunks, dedup %)
#   - cross-repo chunk dedup: two unrelated repos pushing similar content
#     to one remote
#
# Usage: bench/run.sh [out-dir]        (default: ./bench-results)
# Env:   AGENT=<path>       agent binary (default: cargo build --release)
#        SCENARIOS="…"      subset of: big-binary compressible many-files
#                           full-rewrite tensor cross-repo
#        SYSTEMS="…"        subset of: git lfs cavs   (default: all)
#        CAVS_PROFILE=<p>   chunking profile for the agent (label appended
#                           to the system name when set, e.g. cavs-fastcdc-16k)
#        KEEP=1             keep the workdir
set -euo pipefail

command -v git-lfs >/dev/null || { echo "git-lfs required"; exit 1; }

HERE=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$HERE/../../.." && pwd)
OUT=${1:-"$PWD/bench-results"}
mkdir -p "$OUT"
CSV="$OUT/results.csv"
[ -f "$CSV" ] || echo "scenario,system,phase,version,metric,value" > "$CSV"

AGENT=${AGENT:-}
if [ -z "$AGENT" ]; then
  echo "[bench] building cavs-lfs-agent + cavs (release)…"
  (cd "$REPO_ROOT" && cargo build -q --release -p cavs-lfs-agent -p cavs-cli)
  AGENT="$REPO_ROOT/target/release/cavs-lfs-agent"
fi
CAVS_CLI="$REPO_ROOT/target/release/cavs"

CAVS_PROFILE=${CAVS_PROFILE:-}
CAVS_SYS="cavs${CAVS_PROFILE:+-$CAVS_PROFILE}"

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
# Sum sizes (KiB) of files under $1 whose name matches $2.
kb_named() {
  [ -e "$1" ] || { echo 0; return; }
  find "$1" -type f -name "$2" -exec stat "${STAT_SIZE[@]}" {} + 2>/dev/null \
    | awk '{s+=$1} END{printf "%d\n", s/1024}'
}
emit() { echo "$1,$2,$3,$4,$5,$6" >> "$CSV"; }   # scenario system phase version metric value

# sha256 manifest of a tree (relative paths), for correctness checks.
tree_sha() { (cd "$1" && find . -type f ! -path './.git/*' ! -name .gitattributes -exec shasum -a 256 {} + | sort -k2); }

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

track_gitattributes() {
  printf '*.bin filter=lfs diff=lfs merge=lfs -text\n*.pak filter=lfs diff=lfs merge=lfs -text\n' \
    > "$1/.gitattributes"
}

configure_cavs_repo() { # configure_cavs_repo <repo>
  (cd "$1" && git lfs install --local >/dev/null)
  git -C "$1" config lfs.standalonetransferagent cavs
  git -C "$1" config lfs.customtransfer.cavs.path "$AGENT"
  git -C "$1" config lfs.customtransfer.cavs.concurrent false
  if [ -n "$CAVS_PROFILE" ]; then
    git -C "$1" config lfs.customtransfer.cavs.args "--profile $CAVS_PROFILE"
  fi
}

# CAVS storage breakdown of a remote tree -> CSV rows.
emit_cavs_breakdown() { # emit_cavs_breakdown <scenario> <version> <tree>
  local SC=$1 NV=$2 TREE=$3 STORE="$3/.store"
  emit "$SC" "$CAVS_SYS" storage "$NV" store_kb "$(kb "$STORE")"
  emit "$SC" "$CAVS_SYS" storage "$NV" export_kb "$(( $(kb "$TREE") - $(kb "$STORE") ))"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" store_pack_data_kb "$(kb_named "$STORE/packs" '*.cavspack')"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" store_pack_index_kb "$(kb_named "$STORE/packs" '*.cavsindex')"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" store_meta_kb \
    "$(( $(kb "$STORE/assets") + $(kb_named "$STORE" 'index.*') ))"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" export_pack_kb "$(kb "$TREE/chunks/packs")"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" export_index_kb "$(kb "$TREE/chunks/indexes")"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" export_manifest_kb "$(kb_named "$TREE/assets" 'manifest.json')"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" export_chunkmap_kb "$(kb_named "$TREE/assets" 'chunk-map.json')"
  emit "$SC" "$CAVS_SYS" breakdown "$NV" export_record_kb "$(kb_named "$TREE/assets" 'record.json')"
}

# ---------------------------------------------------------------------------
# run_system <scenario> <system>   where system ∈ git | lfs | cavs
# ---------------------------------------------------------------------------
run_system() {
  local SC=$1 SYS=$2 SYSNAME=$2
  [ "$SYS" = cavs ] && SYSNAME=$CAVS_SYS
  local ROOT="$WORK/$SC-$SYSNAME"
  local ORIGIN="$ROOT/origin.git" REPO="$ROOT/repo" TRACK="$ROOT/track" COLD="$ROOT/cold"
  local SHARED_CACHE="$ROOT/shared-cache"
  local NV; NV=$(versions_of "$SC")
  mkdir -p "$ROOT"
  echo "[bench] === $SC / $SYSNAME ($NV versions) ==="

  git init -q -b main --bare "$ORIGIN"
  git init -q -b main "$REPO"
  git -C "$REPO" config user.email bench@example.com
  git -C "$REPO" config user.name bench

  local REMOTE_URL="$ORIGIN" DL_DIR_TRACK= REMOTE_MEASURE="$ORIGIN"
  case $SYS in
    git) ;;
    lfs)
      REMOTE_URL="file://$ORIGIN"
      (cd "$REPO" && git lfs install --local >/dev/null)
      track_gitattributes "$REPO"
      ;;
    cavs)
      configure_cavs_repo "$REPO"
      track_gitattributes "$REPO"
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
    emit "$SC" "$SYSNAME" push "$v" time_s "$(elapsed "$t0" "$t1")"
    emit "$SC" "$SYSNAME" push "$v" remote_kb "$total_kb"
    emit "$SC" "$SYSNAME" push "$v" remote_growth_kb "$((total_kb - prev_kb))"
    prev_kb=$total_kb

    # --- incremental update in the tracking clone ----------------------
    # (CAVS: shared chunk cache via env, also reused by the warm clone)
    if [ "$v" = 1 ]; then
      t0=$(now)
      case $SYS in
        git)  git clone -q "$REMOTE_URL" "$TRACK" ;;
        lfs)  git clone -q "${lfs_filter_cfg[@]}" "$REMOTE_URL" "$TRACK" ;;
        cavs) CAVS_LFS_CACHE="$SHARED_CACHE" \
                git clone -q "${lfs_filter_cfg[@]}" "${cavs_cfg[@]}" \
                ${CAVS_PROFILE:+-c "lfs.customtransfer.cavs.args=--profile $CAVS_PROFILE"} \
                "$REMOTE_URL" "$TRACK"
              configure_cavs_repo "$TRACK" ;;
      esac
      t1=$(now)
      case $SYS in
        git)  DL_DIR_TRACK="$TRACK/.git" ;;
        lfs)  DL_DIR_TRACK="$TRACK/.git/lfs/objects" ;;
        cavs) DL_DIR_TRACK="$SHARED_CACHE" ;;
      esac
    else
      t0=$(now)
      if [ "$SYS" = cavs ]; then
        CAVS_LFS_CACHE="$SHARED_CACHE" git -C "$TRACK" pull -q 2> "$ROOT/pull$v.err"
      else
        git -C "$TRACK" pull -q 2> "$ROOT/pull$v.err"
      fi
      t1=$(now)
    fi
    local dl_kb; dl_kb=$(kb "$DL_DIR_TRACK")
    emit "$SC" "$SYSNAME" update "$v" time_s "$(elapsed "$t0" "$t1")"
    emit "$SC" "$SYSNAME" update "$v" dl_total_kb "$dl_kb"
    # correctness at every step
    if ! diff <(tree_sha "$TRACK") <(tree_sha "$DATA/$SC/v$v") >/dev/null; then
      echo "FAIL: $SC/$SYSNAME v$v tracking clone content mismatch"; exit 1
    fi
  done

  # --- cold clone at latest (fresh cache) --------------------------------
  t0=$(now)
  case $SYS in
    git)  git clone -q "$REMOTE_URL" "$COLD" ;;
    lfs)  git clone -q "${lfs_filter_cfg[@]}" "$REMOTE_URL" "$COLD" ;;
    cavs) git clone -q "${lfs_filter_cfg[@]}" "${cavs_cfg[@]}" \
            ${CAVS_PROFILE:+-c "lfs.customtransfer.cavs.args=--profile $CAVS_PROFILE"} \
            "$REMOTE_URL" "$COLD" ;;
  esac
  t1=$(now)
  local DL_DIR_COLD
  case $SYS in
    git)  DL_DIR_COLD="$COLD/.git" ;;
    lfs)  DL_DIR_COLD="$COLD/.git/lfs/objects" ;;
    cavs) DL_DIR_COLD="$COLD/.git/lfs/cavs/cache" ;;
  esac
  emit "$SC" "$SYSNAME" clone_cold "$NV" time_s "$(elapsed "$t0" "$t1")"
  emit "$SC" "$SYSNAME" clone_cold "$NV" dl_total_kb "$(kb "$DL_DIR_COLD")"
  if ! diff <(tree_sha "$COLD") <(tree_sha "$DATA/$SC/v$NV") >/dev/null; then
    echo "FAIL: $SC/$SYSNAME cold clone content mismatch"; exit 1
  fi
  emit "$SC" "$SYSNAME" verify "$NV" sha256_ok 1

  # --- warm clone: CAVS only — a second machine-level consumer sharing the
  # populated chunk cache. Vanilla LFS has no cross-clone cache (objects are
  # per-repo), so the phase exists only for cavs.
  if [ "$SYS" = cavs ]; then
    local WARM="$ROOT/warm" cache_before
    cache_before=$(kb "$SHARED_CACHE")
    t0=$(now)
    CAVS_LFS_CACHE="$SHARED_CACHE" \
      git clone -q "${lfs_filter_cfg[@]}" "${cavs_cfg[@]}" \
      ${CAVS_PROFILE:+-c "lfs.customtransfer.cavs.args=--profile $CAVS_PROFILE"} \
      "$REMOTE_URL" "$WARM"
    t1=$(now)
    emit "$SC" "$SYSNAME" clone_warm "$NV" time_s "$(elapsed "$t0" "$t1")"
    emit "$SC" "$SYSNAME" clone_warm "$NV" dl_new_kb "$(( $(kb "$SHARED_CACHE") - cache_before ))"
    if ! diff <(tree_sha "$WARM") <(tree_sha "$DATA/$SC/v$NV") >/dev/null; then
      echo "FAIL: $SC/$SYSNAME warm clone content mismatch"; exit 1
    fi
  fi

  # --- system-specific storage breakdown ---------------------------------
  case $SYS in
    cavs)
      emit_cavs_breakdown "$SC" "$NV" "$ORIGIN/cavs"
      "$CAVS_CLI" store "$ORIGIN/cavs/.store" stat > "$OUT/$SC-$SYSNAME-store-stat.txt" 2>&1 || true
      ;;
    lfs)  emit "$SC" "$SYSNAME" storage "$NV" lfs_objects_kb "$(kb "$ORIGIN/lfs")" ;;
    git)  emit "$SC" "$SYSNAME" storage "$NV" git_kb "$(kb "$ORIGIN")" ;;
  esac

  # logical size of the latest version, once per scenario/system
  emit "$SC" "$SYSNAME" logical "$NV" latest_version_kb "$(kb "$DATA/$SC/v$NV")"
  rm -rf "$ROOT"
}

# ---------------------------------------------------------------------------
# Cross-repo dedup: two UNRELATED git repos push similar-but-not-identical
# content (dataset big-binary v1 and v2 — different oids) to one remote.
# Chunk-level dedup should make the second push cheap; vanilla LFS stores
# the second object whole.
# ---------------------------------------------------------------------------
run_crossrepo() {
  local SYS=$1 SYSNAME=$1
  [ "$SYS" = cavs ] && SYSNAME=$CAVS_SYS
  local SC=cross-repo
  local ROOT="$WORK/$SC-$SYSNAME"
  local ORIGIN="$ROOT/origin.git"
  mkdir -p "$ROOT"
  echo "[bench] === $SC / $SYSNAME ==="
  git init -q -b main --bare "$ORIGIN"
  local REMOTE_URL="$ORIGIN"
  [ "$SYS" = lfs ] && REMOTE_URL="file://$ORIGIN"

  local prev_kb=0 r v t0 t1
  for r in a b; do
    v=$([ "$r" = a ] && echo 1 || echo 2)
    local REPO="$ROOT/repo-$r"
    git init -q -b main "$REPO"
    git -C "$REPO" config user.email bench@example.com
    git -C "$REPO" config user.name bench
    case $SYS in
      lfs)  (cd "$REPO" && git lfs install --local >/dev/null); track_gitattributes "$REPO" ;;
      cavs) configure_cavs_repo "$REPO"; track_gitattributes "$REPO" ;;
    esac
    git -C "$REPO" remote add origin "$REMOTE_URL"
    cp -R "$DATA/big-binary/v$v/." "$REPO/"
    git -C "$REPO" add -A
    git -C "$REPO" commit -qm "repo-$r"
    t0=$(now)
    # unrelated histories: each repo pushes its own branch
    git -C "$REPO" push -q origin "main:refs/heads/repo-$r" 2> "$ROOT/push-$r.err"
    t1=$(now)
    local total_kb; total_kb=$(kb "$ORIGIN")
    emit "$SC" "$SYSNAME" push-"$r" 1 time_s "$(elapsed "$t0" "$t1")"
    emit "$SC" "$SYSNAME" push-"$r" 1 remote_kb "$total_kb"
    emit "$SC" "$SYSNAME" push-"$r" 1 remote_growth_kb "$((total_kb - prev_kb))"
    prev_kb=$total_kb
  done

  # verify repo-b's object round-trips from the shared remote
  local CHECK="$ROOT/check"
  case $SYS in
    lfs)  git clone -q -b repo-b "${lfs_filter_cfg[@]}" "$REMOTE_URL" "$CHECK" ;;
    cavs) git clone -q -b repo-b "${lfs_filter_cfg[@]}" "${cavs_cfg[@]}" \
            ${CAVS_PROFILE:+-c "lfs.customtransfer.cavs.args=--profile $CAVS_PROFILE"} \
            "$REMOTE_URL" "$CHECK" ;;
  esac
  if ! diff <(tree_sha "$CHECK") <(tree_sha "$DATA/big-binary/v2") >/dev/null; then
    echo "FAIL: $SC/$SYSNAME repo-b content mismatch"; exit 1
  fi
  emit "$SC" "$SYSNAME" verify 1 sha256_ok 1
  if [ "$SYS" = cavs ]; then
    emit_cavs_breakdown "$SC" 1 "$ORIGIN/cavs"
  fi
  rm -rf "$ROOT"
}

SCENARIOS=${SCENARIOS:-"big-binary compressible many-files full-rewrite tensor cross-repo"}
SYSTEMS=${SYSTEMS:-"git lfs cavs"}
for SC in $SCENARIOS; do
  if [ "$SC" = cross-repo ]; then
    for SYS in $SYSTEMS; do
      [ "$SYS" = git ] && continue   # cross-repo is an LFS-vs-CAVS question
      run_crossrepo "$SYS"
    done
    continue
  fi
  for SYS in $SYSTEMS; do
    run_system "$SC" "$SYS"
  done
done

echo
echo "[bench] done — raw metrics in $CSV"
