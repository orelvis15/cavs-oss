# CAVS Kotlin SDK — examples

Runnable examples for the Kotlin/JVM SDK. They live in their own Gradle source
set (`examples/`) so they never end up in the published library jar, and run
via the `runQuickstart` / `runPreview` tasks.

## Prerequisites

- **Java 22+** (the native bridge uses the FFM API, [JEP 454](https://openjdk.org/jeps/454)).
  Gradle can auto-provision a Java 22 toolchain if the runner doesn't have one.
- The native library, pointed to via `CAVS_SDK_LIBRARY`:

```sh
# from the repo root
cargo build --release -p cavs-ffi
export CAVS_SDK_LIBRARY="$PWD/target/release/libcavs_sdk.dylib"   # .so on Linux, .dll on Windows
```

Run the commands below from `sdks/kotlin`.

## Examples

### `runQuickstart` — the whole lifecycle, zero setup

Generates two synthetic builds in a temp directory, then walks the full update
flow: **analyze → preview → createPlan → applyPlan → estimateSavings**. Nothing
to download, nothing to clean up.

```sh
gradle runQuickstart
```

You'll see how much of the new build is reused from the old one, the wire cost
of each delivery route, a `.cavsplan` being written and applied back to
reconstruct the new build, and a rough egress-savings estimate at scale.

### `runPreview` — one call against your own builds

Runs just the update preview between two directories you already have on disk.
Pass the paths through `--args`:

```sh
gradle runPreview --args="--old /path/to/Build_v1 --new /path/to/Build_v2"
```

## Notes

- The run tasks enable native access (`--enable-native-access=ALL-UNNAMED`) and
  forward `CAVS_SDK_LIBRARY` to the JVM for you, and they run on the Java 22
  toolchain launcher (the example classes are compiled for Java 22).
- Add `-q` to silence Gradle's own logging and see only the example output.

## How the sample builds are made

`runQuickstart` uses the helper in `Sample.kt`. It writes a v1 build and a v2
derived from it with a realistic mix of changes — files that stay identical, one
patched in place, one added, one removed — using large, repetitive payloads so
CAVS' chunk reuse is easy to see. Edit `Sample.kt` if you want to shape the data
differently.
