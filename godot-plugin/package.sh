#!/bin/sh
# Empaqueta el plugin de Godot como addon instalable.
# Salida: dist/cavs-godot-plugin-<version>.zip
# Instalación en un proyecto Godot 4:
#   Proyecto > Herramientas > ... o simplemente descomprimir en la raíz del
#   proyecto (crea addons/cavs/) y activar en Ajustes del proyecto > Plugins.
set -eu
cd "$(dirname "$0")"

VERSION=$(grep '^version=' addons/cavs/plugin.cfg | cut -d'"' -f2)
OUT_DIR=dist
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$OUT_DIR"
mkdir -p "$STAGE/addons/cavs"
cp addons/cavs/* "$STAGE/addons/cavs/"
cp README.md "$STAGE/addons/cavs/README.md"
cp ../../LICENSE "$STAGE/addons/cavs/LICENSE"

ZIP="$PWD/$OUT_DIR/cavs-godot-plugin-$VERSION.zip"
rm -f "$ZIP"
(cd "$STAGE" && zip -q -r "$ZIP" addons)
echo "listo: $ZIP"
unzip -l "$ZIP"
