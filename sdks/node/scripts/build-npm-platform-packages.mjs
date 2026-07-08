// Generate the per-platform npm packages (@orelvis15/cavs-sdk-<os>-<arch>)
// that carry the native library, from the release's native artifacts.
//
// Usage: node scripts/build-npm-platform-packages.mjs <artifacts-dir> <version>
//
// <artifacts-dir> holds the extracted `cavs-sdk-native-<version>-<target>/`
// directories produced by the release workflow's sdk-native job. Output is
// written to `npm/<pkg>/` ready for `npm publish`.
import { cpSync, existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [artifactsDir, version] = process.argv.slice(2);
if (!artifactsDir || !version) {
  console.error("usage: build-npm-platform-packages.mjs <artifacts-dir> <version>");
  process.exit(2);
}

// target triple -> { os, cpu (npm), lib file }
const TARGETS = {
  "x86_64-unknown-linux-gnu": { os: "linux", cpu: "x64", lib: "libcavs_sdk.so" },
  "aarch64-unknown-linux-gnu": { os: "linux", cpu: "arm64", lib: "libcavs_sdk.so" },
  "x86_64-apple-darwin": { os: "darwin", cpu: "x64", lib: "libcavs_sdk.dylib" },
  "aarch64-apple-darwin": { os: "darwin", cpu: "arm64", lib: "libcavs_sdk.dylib" },
  "x86_64-pc-windows-msvc": { os: "win32", cpu: "x64", lib: "cavs_sdk.dll" },
};

const outRoot = join(process.cwd(), "npm");
mkdirSync(outRoot, { recursive: true });

let built = 0;
for (const [target, info] of Object.entries(TARGETS)) {
  // The release's sdk-native job names artifact dirs from the raw tag, which
  // may carry a leading "v" (cavs-sdk-native-v1.2.0-<target>); accept both.
  let srcDir = join(artifactsDir, `cavs-sdk-native-${version}-${target}`);
  if (!existsSync(join(srcDir, info.lib))) {
    srcDir = join(artifactsDir, `cavs-sdk-native-v${version}-${target}`);
  }
  const libPath = join(srcDir, info.lib);
  if (!existsSync(libPath)) {
    console.warn(`skip ${target}: ${info.lib} not found for ${target}`);
    continue;
  }
  const pkgName = `@orelvis15/cavs-sdk-${info.os}-${info.cpu}`;
  const pkgDir = join(outRoot, `${info.os}-${info.cpu}`);
  const nativeDir = join(pkgDir, "native");
  mkdirSync(nativeDir, { recursive: true });
  cpSync(libPath, join(nativeDir, info.lib));

  const pkg = {
    name: pkgName,
    version,
    description: `CAVS native library for ${info.os}-${info.cpu}`,
    license: "Apache-2.0",
    os: [info.os],
    cpu: [info.cpu],
    files: ["native"],
  };
  writeFileSync(join(pkgDir, "package.json"), JSON.stringify(pkg, null, 2) + "\n");
  console.log(`built ${pkgName} (${info.lib})`);
  built++;
}

if (built === 0) {
  console.error("no platform packages built — check the artifacts directory");
  process.exit(1);
}

// Confirm the sibling packages listed in native.ts resolution.
const files = readdirSync(outRoot);
console.log(`\n${built} platform package(s) under npm/: ${files.join(", ")}`);
