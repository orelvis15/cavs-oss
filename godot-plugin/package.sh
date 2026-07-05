#!/bin/sh
# Empaqueta el plugin de Godot como addon instalable para la Godot Asset Store.
# Salida: dist/cavs-godot-plugin-<version>.zip con `addons/cavs/` en la raíz del
# zip (que es lo que exige el formulario "nueva versión" de la store).
#
# Instalación en un proyecto Godot 4:
#   Descomprimir en la raíz del proyecto (crea addons/cavs/) y activar en
#   Ajustes del proyecto > Plugins.
#
# Uso:  ./package.sh            # versión tomada de addons/cavs/plugin.cfg
#       ./package.sh 0.1.2      # sobrescribe la versión del nombre del zip
set -eu
cd "$(dirname "$0")"

VERSION="${1:-$(grep '^version=' addons/cavs/plugin.cfg | cut -d'"' -f2)}"
OUT_DIR=dist
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$OUT_DIR"
mkdir -p "$STAGE/addons/cavs"

# Copiar solo los archivos del addon explícitamente (evita .DS_Store y basura).
for f in cavs_client.gd plugin.gd plugin.cfg icon.png LICENSE; do
    cp "addons/cavs/$f" "$STAGE/addons/cavs/$f"
done
# README del plugin, documentado dentro del propio addon.
cp README.md "$STAGE/addons/cavs/README.md"

ZIP="$PWD/$OUT_DIR/cavs-godot-plugin-$VERSION.zip"
rm -f "$ZIP"
(cd "$STAGE" && zip -q -r "$ZIP" addons)
echo "listo: $ZIP"
unzip -l "$ZIP"
