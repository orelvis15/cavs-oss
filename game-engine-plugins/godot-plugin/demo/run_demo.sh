#!/bin/sh
# Demo Godot reproducible de CAVS, de cero a ventana:
#   1. compila los binarios release de CAVS
#   2. genera un mini-juego Godot y exporta game_v1.pck / game_v2.pck reales
#   3. los empaqueta como .cavs (FastCDC 64 KiB + zstd, default)
#   4. levanta cavs-server en 127.0.0.1:8991
#   5. abre la demo: instala v1 en frío y actualiza a v2 descargando solo lo
#      que cambió, con barra de progreso y verificación sha256 en pantalla
#
# Requisitos: rust (cargo), godot 4.x y ffmpeg en el PATH.
# Con DEMO_HEADLESS=1 valida el flujo sin abrir ventana (para CI).
set -eu

DEMO_DIR=$(cd "$(dirname "$0")" && pwd)
CAVS_ROOT=$(cd "$DEMO_DIR/../.." && pwd)
WORK="$DEMO_DIR/.work"
PORT=8991

command -v godot >/dev/null || { echo "necesitas godot en el PATH"; exit 1; }
command -v ffmpeg >/dev/null || { echo "necesitas ffmpeg en el PATH"; exit 1; }
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

echo "==> [1/5] compilando binarios release..."
(cd "$CAVS_ROOT" && cargo build --release -q -p cavs-cli -p cavs-server)
CAVS="$CAVS_ROOT/target/release/cavs"
SERVER="$CAVS_ROOT/target/release/cavs-server"

echo "==> [2/5] generando mini-juego y exportando PCKs v1/v2..."
rm -rf "$WORK"
mkdir -p "$WORK/game/textures" "$WORK/game/levels"

make_texture() { # $1 nombre  $2 lavfi
    ffmpeg -y -hide_banner -loglevel error -f lavfi -i "$2" -frames:v 1 \
        "$WORK/game/textures/$1"
}
write_level() { # $1 numero  $2 textura
    cat > "$WORK/game/levels/level_$1.tscn" <<EOF
[gd_scene load_steps=2 format=3]

[ext_resource type="Texture2D" path="res://textures/$2" id="1"]

[node name="Level$1" type="Node2D"]

[node name="Background" type="Sprite2D" parent="."]
texture = ExtResource("1")
EOF
}

cat > "$WORK/game/project.godot" <<'EOF'
config_version=5

[application]

config/name="CavsDemoGame"
config/features=PackedStringArray("4.7")
EOF
cat > "$WORK/game/export_presets.cfg" <<'EOF'
[preset.0]

name="pck"
platform="Linux"
runnable=true
export_filter="all_resources"
include_filter=""
exclude_filter=""
export_path=""

[preset.0.options]
EOF

export_pck() { # $1 salida
    godot --headless --path "$WORK/game" --import >/dev/null 2>&1 || true
    godot --headless --path "$WORK/game" --export-pack pck "$1" >/dev/null 2>&1
    [ -s "$1" ] || { echo "export de PCK falló"; exit 1; }
}

# v1: un juego realista — muchos assets estables (texturas + audio) y niveles.
# Un parche real toca POCOS assets; así el dedupe tiene contenido compartido
# que reutilizar, como en los juegos reales del benchmark.
make_texture t1.png "mandelbrot=size=1024x1024:maxiter=160"
make_texture t2.png "mandelbrot=size=1024x1024:maxiter=224"
make_texture t3.png "mandelbrot=size=1024x1024:maxiter=288"
make_texture t4.png "gradients=size=1024x1024:seed=7:speed=0.01"
make_texture t5.png "gradients=size=1024x1024:seed=21:speed=0.01"
make_texture t6.png "testsrc2=size=1024x1024:rate=30"
ffmpeg -y -hide_banner -loglevel error -f lavfi -i "sine=frequency=440:duration=12" "$WORK/game/theme_a.wav"
ffmpeg -y -hide_banner -loglevel error -f lavfi -i "sine=frequency=659:duration=12" "$WORK/game/theme_b.wav"
write_level 1 t1.png
write_level 2 t2.png
write_level 3 t4.png
write_level 4 t6.png
export_pck "$WORK/game_v1.pck"

# v2: cambia SOLO t1, añade t_new + level_5 (un parche típico)
make_texture t1.png "mandelbrot=size=1024x1024:maxiter=420"
make_texture t_new.png "gradients=size=1024x1024:seed=99:speed=0.01"
write_level 5 t_new.png
export_pck "$WORK/game_v2.pck"
echo "    game_v1.pck: $(du -h "$WORK/game_v1.pck" | cut -f1) · game_v2.pck: $(du -h "$WORK/game_v2.pck" | cut -f1)"

echo "==> [3/5] empaquetando releases como .cavs..."
"$CAVS" pack --raw "$WORK/game_v1.pck" -o "$WORK/game_v1.cavs" >/dev/null
"$CAVS" pack --raw "$WORK/game_v2.pck" -o "$WORK/game_v2.cavs" >/dev/null

echo "==> [4/5] cavs-server en 127.0.0.1:$PORT..."
"$SERVER" "$WORK/game_v1.cavs" "$WORK/game_v2.cavs" --listen "127.0.0.1:$PORT" \
    > "$WORK/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT
sleep 1
kill -0 $SERVER_PID 2>/dev/null || { echo "el servidor no arrancó"; cat "$WORK/server.log"; exit 1; }

echo "==> [5/5] preparando proyecto demo..."
mkdir -p "$DEMO_DIR/addons"
cp -r "$DEMO_DIR/../addons/cavs" "$DEMO_DIR/addons/" 2>/dev/null || true
rm -rf "$DEMO_DIR/.godot"

if [ "${DEMO_HEADLESS:-0}" = "1" ]; then
    echo "==> validación headless (frío v1 -> update v2)..."
    cat > "$WORK/headless_check.gd" <<EOF
extends SceneTree
const C := preload("res://addons/cavs/cavs_client.gd")
func _wipe(p):
    var d := DirAccess.open(p)
    if d == null: return
    for f in d.get_files(): d.remove(f)
    for s in d.get_directories(): _wipe(p + "/" + s); d.remove(s)
func _init():
    var c = C.new("http://127.0.0.1:$PORT")
    _wipe(c.cache_dir)
    var a = c.fetch("game_v1")
    var b = c.fetch("game_v2")
    print("V1_WIRE=", a.bytes_wire, " V2_WIRE=", b.bytes_wire, " OK=", a.ok and b.ok)
    quit(0 if (a.ok and b.ok and b.bytes_wire < a.bytes_wire / 2) else 1)
EOF
    OUT=$(godot --headless --path "$DEMO_DIR" -s "$WORK/headless_check.gd" 2>/dev/null) || {
        echo "$OUT"; echo "validación FALLÓ"; exit 1; }
    echo "$OUT" | grep "V1_WIRE"
    echo "demo validada ✅ (el update viajó como fracción de la instalación)"
else
    echo ""
    echo "    Abriendo la demo (cierra la ventana para terminar)."
    echo "    ① Instalar v1 = descarga fría · ② Actualizar a v2 = solo lo que cambió"
    godot --path "$DEMO_DIR" 2>/dev/null || true
fi
