#!/bin/sh
# Publica una release de un juego Godot con CAVS (lado CI/build):
#   1. exporta el PCK con el editor headless
#   2. lo empaqueta como .cavs (FastCDC + zstd + firma opcional)
#   3. queda listo para servir con cavs-server
#
# Uso: ./pack_release.sh <proyecto_godot> <preset> <nombre_release> [sign_key]
# Ej.: ./pack_release.sh ~/mi-juego pck game_v42 keys/publisher.key
set -eu

PROJECT=${1:?proyecto godot}
PRESET=${2:?nombre del preset de export}
RELEASE=${3:?nombre de la release (ej. game_v42)}
SIGN_KEY=${4:-}

OUT_DIR=$(pwd)/releases
mkdir -p "$OUT_DIR"
PCK="$OUT_DIR/$RELEASE.pck"
CAVS_FILE="$OUT_DIR/$RELEASE.cavs"

echo "==> exportando PCK ($PRESET)..."
godot --headless --path "$PROJECT" --import >/dev/null 2>&1 || true
godot --headless --path "$PROJECT" --export-pack "$PRESET" "$PCK"

echo "==> empaquetando con CAVS (FastCDC 64K/256K/1M + zstd)..."
if [ -n "$SIGN_KEY" ]; then
    cavs pack --raw --mode cdc --sign-key "$SIGN_KEY" "$PCK" -o "$CAVS_FILE"
else
    cavs pack --raw --mode cdc "$PCK" -o "$CAVS_FILE"
fi

cavs verify "$CAVS_FILE"
echo ""
echo "listo: $CAVS_FILE"
echo "servir:  cavs-server $OUT_DIR/*.cavs --listen 0.0.0.0:8990"
echo "runtime: CavsClient.new(\"https://tu-servidor\").ensure_pack(\"$RELEASE\")"
