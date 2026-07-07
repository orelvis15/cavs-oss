#!/bin/sh
# v1.0.0 certification benchmark suite — regenerates every report in
# docs/results/v1.0.0 from deterministic synthetic datasets.
#
# Needs: a release build of the cavs CLI on PATH (cargo build --release).
# Optional: butler / xdelta3 / bsdiff on PATH for the external comparisons
# (they are skipped, never failed, when missing).
set -e

WORK=${1:-./v1-bench}
mkdir -p "$WORK"
cd "$WORK"

# ---- Dataset (groups A, B, C, H) -------------------------------------------
cavs bench gen-dir --out dataset --size 128MiB --seed 7

# ---- Full strict certification + regression baseline (A, B, H, J) ----------
cavs certify \
  --old dataset/Build_v1 \
  --new dataset/Build_v2 \
  --profile strict \
  --save-baseline baseline.json \
  --out ./certification

# ---- CI profile against the baseline (C) ------------------------------------
cavs certify \
  --old dataset/Build_v1 \
  --new dataset/Build_v2 \
  --profile ci \
  --baseline baseline.json \
  --json-out certification.json \
  --out ./certification-ci || test $? -eq 2   # layout warnings are expected

# ---- SteamPipe-style case matrix, kept datasets (E, G sources) --------------
cavs bench steampipe-cases --out steampipe-cases --seed 9 --keep-datasets

# ---- Godot PCK certification (D) ---------------------------------------------
cavs certify godot \
  --old-pck steampipe-cases/datasets/godot-pck-localized/old/game.pck \
  --new-pck steampipe-cases/datasets/godot-pck-localized/new/game.pck \
  --out ./cert-godot

# ---- Workspace / depot / install-plan certification (I) -----------------------
mkdir -p ws/windows ws/linux ws/lang-es ws/dlc1
cp -r dataset/Build_v1 ws/base-v1
cp -r dataset/Build_v2 ws/base-v2
head -c 4000000 dataset/Build_v1/game.pck > ws/windows/win.bin
head -c 4000000 dataset/Build_v1/game.pck > ws/linux/linux.bin
head -c 300000  dataset/Build_v1/game.pck > ws/lang-es/es.pak
head -c 8000000 dataset/Build_v2/game.pck > ws/dlc1/dlc1.pak
cavs workspace init ./cavs-workspace --app my-game
cavs depot add base    --workspace ./cavs-workspace
cavs depot add windows --platform windows --workspace ./cavs-workspace
cavs depot add linux   --platform linux   --workspace ./cavs-workspace
cavs depot add lang-es --language es --optional --workspace ./cavs-workspace
cavs depot add dlc1    --optional --workspace ./cavs-workspace
cavs branch add beta --workspace ./cavs-workspace
cavs build create --workspace ./cavs-workspace --branch beta \
  --depot base=./ws/base-v1 --depot windows=./ws/windows --depot linux=./ws/linux \
  --depot lang-es=./ws/lang-es --depot dlc1=./ws/dlc1 --label build_1001
cavs build create --workspace ./cavs-workspace --branch beta \
  --depot base=./ws/base-v2 --depot windows=./ws/windows --depot linux=./ws/linux \
  --depot lang-es=./ws/lang-es --depot dlc1=./ws/dlc1 --label build_1002
cavs certify workspace \
  --workspace ./cavs-workspace \
  --from build_1001 --to build_1002 \
  --out ./cert-ws

echo
echo "Reports: certification/ certification-ci/ cert-godot/ cert-ws/ steampipe-cases/"
