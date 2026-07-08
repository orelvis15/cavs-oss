# CAVS Java SDK

Java bindings for CAVS. The SDK loads the same compiled Rust core the CAVS
CLI uses through a stable C ABI — it does not shell out to the CLI.

## Requirements

- **Java 22+** (the native bridge uses the Foreign Function & Memory API,
  [JEP 454](https://openjdk.org/jeps/454), finalized in Java 22).
- The native library `libcavs_sdk.{so,dylib}` / `cavs_sdk.dll`.

The native backend sits behind a `NativeBridge` interface, so a JNA-based
backend for the Java 17 baseline can be added without changing `CavsClient`.

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

```java
import com.cavs.sdk.*;
import com.cavs.sdk.model.*;

try (CavsClient cavs = CavsClient.create()) {
    PreviewReport preview = cavs.preview(
        PreviewRequest.builder()
            .oldPath("Build_v1")
            .newPath("Build_v2")
            .policy("balanced")
            .build());
    System.out.println("Recommended route: " + preview.recommendedRoute());
}
```

Run with native access enabled:

```sh
java --enable-native-access=ALL-UNNAMED -jar app.jar
```

## API

`CavsClient` (an `AutoCloseable`) exposes `analyze`, `packDirectory`,
`preview`, `createPlan`, `applyPlan`, `verifyInstall`, `benchmark` and
`estimateSavings`, each with an optional `Consumer<ProgressEvent>` overload,
plus `previewAsync`, `createPlanAsync` and `applyPlanAsync` returning
`CompletableFuture`. Failures throw `CavsException` carrying a
`CavsErrorCode`.

## Build & test

Gradle:

```sh
./gradlew test           # requires a Java 22 toolchain + CAVS_SDK_LIBRARY
```

Maven:

```sh
mvn -B verify
```

Both wire `--enable-native-access=ALL-UNNAMED` and pass `CAVS_SDK_LIBRARY`
through to the test JVM.
```
