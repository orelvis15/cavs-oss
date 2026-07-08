# CAVS Kotlin SDK

Kotlin/JVM bindings for CAVS. The SDK loads the same compiled Rust core the
CAVS CLI uses through a stable C ABI — it does not shell out to the CLI.

## Requirements

- **Java 22+** (the native bridge uses the Foreign Function & Memory API,
  [JEP 454](https://openjdk.org/jeps/454), finalized in Java 22).
- The native library `libcavs_sdk.{so,dylib}` / `cavs_sdk.dll`.

The native backend sits behind a `NativeBridge` interface, so an alternative
backend can slot in without changing `CavsClient`.

## Native library

Released artifacts bundle the platform library under
`src/main/resources/native/<os>-<arch>/`. For local development, build it
from the Rust workspace and point the loader at it:

```sh
cargo build --release -p cavs-ffi          # from the repo root
export CAVS_SDK_LIBRARY="$PWD/target/release/libcavs_sdk.dylib"   # or .so
```

`-Dcavs.sdk.library=/path/to/lib` works too.

## Quickstart

```kotlin
import com.cavs.sdk.CavsClient
import com.cavs.sdk.model.PreviewRequest

CavsClient.create().use { cavs ->
    val preview = cavs.preview(
        PreviewRequest(oldPath = "Build_v1", newPath = "Build_v2", policy = "balanced"),
    )
    println("Recommended route: ${preview.recommendedRoute}")
}
```

Run with native access enabled:

```sh
java --enable-native-access=ALL-UNNAMED -jar app.jar
```

## API

`CavsClient` (an `AutoCloseable`) exposes `analyze`, `packDirectory`,
`preview`, `createPlan`, `applyPlan`, `verifyInstall`, `benchmark` and
`estimateSavings`. Each operation takes a typed request and an optional
`(ProgressEvent) -> Unit` progress lambda; `previewAsync`, `createPlanAsync`
and `applyPlanAsync` return `CompletableFuture`. Failures throw
`CavsException` carrying a `CavsErrorCode`.

Requests and responses are Kotlin `data class`es serialized with
kotlinx.serialization; property names map directly to the engine's JSON.

## Examples

Runnable examples live in [`examples/`](examples/). `runQuickstart` generates
two synthetic builds and walks the full lifecycle (analyze → preview →
createPlan → applyPlan → estimateSavings) with zero setup:

```sh
export CAVS_SDK_LIBRARY="/path/to/libcavs_sdk.dylib"   # or .so / .dll
gradle runQuickstart
```

See [`examples/README.md`](examples/README.md) for the full list.

## Build & test

Gradle (primary):

```sh
gradle test           # requires a Java 22 toolchain + CAVS_SDK_LIBRARY
```

Maven:

```sh
mvn -B verify
```

Both wire `--enable-native-access=ALL-UNNAMED` and pass `CAVS_SDK_LIBRARY`
through to the test JVM.
