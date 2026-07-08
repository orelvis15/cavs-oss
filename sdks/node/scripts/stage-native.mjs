// Copies the freshly built native library + header from the Rust workspace
// into sdks/node/native/ so the SDK loads it during local development.
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const nativeDir = join(here, "..", "native");
mkdirSync(nativeDir, { recursive: true });

const lib =
  process.platform === "darwin"
    ? "libcavs_sdk.dylib"
    : process.platform === "win32"
      ? "cavs_sdk.dll"
      : "libcavs_sdk.so";

copyFileSync(join(root, "target", "release", lib), join(nativeDir, lib));
copyFileSync(join(root, "core", "cavs-ffi", "include", "cavs_sdk.h"), join(nativeDir, "cavs_sdk.h"));
console.log(`staged ${lib} + cavs_sdk.h into native/`);
