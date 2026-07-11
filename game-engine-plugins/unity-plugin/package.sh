#!/bin/sh
# Package the CAVS Unity plugin as an installable UPM zip, with the native
# libcavs_sdk libraries bundled under Plugins/ so it is drop-in ready.
#
# Output: dist/cavs-unity-plugin-<version>.zip containing the UPM package
# (package.json at the zip root), which a user copies into their project's
# Packages/ folder (or adds via "Add package from disk").
#
# NOTE: this packages the plugin; it does not compile the C# inside a Unity
# Editor (that requires a licensed Unity install). The plugin is currently
# marked UNTESTED — see README.md.
#
# Usage:
#   ./package.sh [version] [native-extracted-dir]
#     version              defaults to package.json's "version"
#     native-extracted-dir a directory holding the extracted
#                          cavs-sdk-native-<ver>-<target> folders from the
#                          sdk-native build; when given, the platform
#                          libraries are staged under Plugins/. Omit to ship
#                          a source-only package (user supplies the natives).
set -eu
cd "$(dirname "$0")"

VERSION="${1:-$(grep '"version"' package.json | head -1 | cut -d'"' -f4)}"
NATIVE_DIR="${2:-}"
OUT_DIR=dist
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

PKG="$STAGE/com.cavs.sdk"
mkdir -p "$PKG"

# The UPM package payload (explicit list keeps junk out).
cp package.json README.md "$PKG/"
cp -R Runtime "$PKG/Runtime"
cp -R "Samples~" "$PKG/Samples~"
mkdir -p "$PKG/Plugins"
cp Plugins/README.md "$PKG/Plugins/" 2>/dev/null || true

# Stamp the version into the packaged package.json.
sed "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" package.json > "$PKG/package.json"

# Stage native libraries, if provided, into Unity's per-platform Plugins dirs.
if [ -n "$NATIVE_DIR" ] && [ -d "$NATIVE_DIR" ]; then
  stage() { # <target-substring> <dest-subdir> <glob>
    src=$(find "$NATIVE_DIR" -type d -name "cavs-sdk-native-*-$1" | head -1)
    [ -n "$src" ] || return 0
    mkdir -p "$PKG/Plugins/$2"
    # shellcheck disable=SC2231
    for f in "$src"/$3; do [ -e "$f" ] && cp "$f" "$PKG/Plugins/$2/"; done
  }
  stage x86_64-pc-windows-msvc   "x86_64/Windows"      "cavs_sdk.dll"
  stage x86_64-apple-darwin      "macOS"               "libcavs_sdk.dylib"
  stage aarch64-apple-darwin     "macOS"               "libcavs_sdk.dylib"
  stage x86_64-unknown-linux-gnu "x86_64/Linux"        "libcavs_sdk.so"
  stage aarch64-unknown-linux-gnu "aarch64/Linux"      "libcavs_sdk.so"
  echo "staged native libraries from $NATIVE_DIR"
else
  echo "no native dir given: shipping a source-only package (see Plugins/README.md)"
fi

mkdir -p "$OUT_DIR"
ZIP="$PWD/$OUT_DIR/cavs-unity-plugin-$VERSION.zip"
rm -f "$ZIP"
(cd "$STAGE" && zip -q -r "$ZIP" com.cavs.sdk)
echo "listo: $ZIP"
unzip -l "$ZIP" | tail -20
