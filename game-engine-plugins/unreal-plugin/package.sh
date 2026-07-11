#!/bin/sh
# Package the CAVS Unreal plugin as an installable zip, with the native
# libcavs_sdk libraries bundled under Source/ThirdParty/CavsSdkLibrary/lib/
# so it is drop-in ready.
#
# Output: dist/cavs-unreal-plugin-<version>.zip containing the plugin folder
# (CavsSdk.uplugin at its root), which a user copies into their project's
# Plugins/CavsSdk/ folder.
#
# NOTE: this packages the plugin; it does not build the C++ with UnrealBuildTool
# (that requires an Unreal Engine install). The plugin is currently marked
# UNTESTED — see README.md.
#
# Usage:
#   ./package.sh [version] [native-extracted-dir]
#     version              defaults to the .uplugin's "VersionName"
#     native-extracted-dir a directory holding the extracted
#                          cavs-sdk-native-<ver>-<target> folders from the
#                          sdk-native build; the platform libraries are staged
#                          under Source/ThirdParty/CavsSdkLibrary/lib/<Platform>/.
set -eu
cd "$(dirname "$0")"

VERSION="${1:-$(grep '"VersionName"' CavsSdk.uplugin | head -1 | cut -d'"' -f4)}"
NATIVE_DIR="${2:-}"
OUT_DIR=dist
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

PLUG="$STAGE/CavsSdk"
mkdir -p "$PLUG"

# Payload: the uplugin descriptor, README, and the whole Source tree
# (module rules, C++ sources, the vendored C ABI header).
cp CavsSdk.uplugin README.md "$PLUG/"
cp -R Source "$PLUG/Source"
# Drop any local build junk.
find "$PLUG" -name '.DS_Store' -delete 2>/dev/null || true

# Stage native libraries, if provided, under lib/<UnrealPlatform>/.
if [ -n "$NATIVE_DIR" ] && [ -d "$NATIVE_DIR" ]; then
  LIBROOT="$PLUG/Source/ThirdParty/CavsSdkLibrary/lib"
  stage() { # <target-substring> <UnrealPlatform> <files...>
    src=$(find "$NATIVE_DIR" -type d -name "cavs-sdk-native-*-$1" | head -1)
    [ -n "$src" ] || return 0
    mkdir -p "$LIBROOT/$2"
    shift 2
    for f in "$@"; do [ -e "$src/$f" ] && cp "$src/$f" "$LIBROOT/$2/"; done
  }
  stage x86_64-pc-windows-msvc   Win64 cavs_sdk.dll cavs_sdk.dll.lib
  stage x86_64-apple-darwin      Mac   libcavs_sdk.dylib
  stage x86_64-unknown-linux-gnu Linux libcavs_sdk.so
  echo "staged native libraries from $NATIVE_DIR"
else
  echo "no native dir given: shipping a source-only package (see Source/ThirdParty/CavsSdkLibrary/README.md)"
fi

mkdir -p "$OUT_DIR"
ZIP="$PWD/$OUT_DIR/cavs-unreal-plugin-$VERSION.zip"
rm -f "$ZIP"
(cd "$STAGE" && zip -q -r "$ZIP" CavsSdk)
echo "listo: $ZIP"
unzip -l "$ZIP" | tail -20
