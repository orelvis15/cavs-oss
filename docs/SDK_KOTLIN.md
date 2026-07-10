# CAVS Kotlin SDK (v1.2.0)

Kotlin/JVM bindings for CAVS. The SDK loads the same compiled Rust core the
CAVS CLI uses through a stable C ABI — it does not shell out to the CLI. The
native bridge uses the Java Foreign Function & Memory API (FFM). See
[SDKS.md](SDKS.md) for the shared architecture, envelope, operations and error
model.

- Coordinates: `io.github.orelvis15:cavs-sdk:1.2.0`
- Package: `com.cavs.sdk` (models in `com.cavs.sdk.model`)

## Requirements

- **Java 22+.** The native bridge uses the Foreign Function & Memory API
  ([JEP 454](https://openjdk.org/jeps/454)), finalized in Java 22.
- The native library (`libcavs_sdk.{so,dylib}` / `cavs_sdk.dll`).
- FFM downcalls/upcalls require native access to be enabled at runtime:
  `--enable-native-access=ALL-UNNAMED`.

The native backend sits behind a `NativeBridge` interface (the shipped
implementation is `FfmNativeBridge`), so an alternative backend can slot in
without changing `CavsClient`.

## Install

Gradle (Kotlin DSL):

```kotlin
dependencies {
    implementation("io.github.orelvis15:cavs-sdk:1.2.0")
}
```

Maven:

```xml
<dependency>
  <groupId>io.github.orelvis15</groupId>
  <artifactId>cavs-sdk</artifactId>
  <version>1.2.0</version>
</dependency>
```

Requests and responses are Kotlin `data class`es serialized with
kotlinx.serialization; property names map directly to the engine's JSON.

## Native library setup

Released artifacts bundle the platform library under
`src/main/resources/native/<os>-<arch>/` (e.g. `linux-x86_64`,
`macos-aarch64`, `windows-x86_64`) and the loader extracts and, when a
`.sha256` sidecar is present, checksum-verifies it before loading.

For local development, build it from the Rust workspace and point the loader
at it:

```sh
cargo build --release -p cavs-ffi          # from the repo root
export CAVS_SDK_LIBRARY="$PWD/target/release/libcavs_sdk.dylib"   # or .so / .dll
```

The loader checks, in order: the `-Dcavs.sdk.library=/path/to/lib` system
property, the `CAVS_SDK_LIBRARY` environment variable, then the bundled jar
resource. `-Dcavs.sdk.library=…` and `CAVS_SDK_LIBRARY` are equivalent.

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

## Client lifecycle

`CavsClient` is an `AutoCloseable` that owns a native context. Use it inside a
`use { }` block, or `close()` it explicitly. It is safe to share across
threads — calls are serialized so a per-call progress sink is never clobbered
by a concurrent call. Calls after close throw `IllegalStateException`.

```kotlin
fun create(): CavsClient
fun create(options: CavsOptions): CavsClient   // e.g. a custom NativeBridge for tests
```

`CavsOptions.builder().bridge(myBridge).build()` swaps in an alternative
`NativeBridge` (e.g. a test double).

Metadata accessors:

```kotlin
fun version(): String     // native SDK version
fun abiVersion(): String  // native C ABI contract version
```

## API

Each synchronous operation takes a typed request and an optional
`(ProgressEvent) -> Unit` progress lambda:

```kotlin
fun analyze(request: AnalyzeRequest, progress: ((ProgressEvent) -> Unit)? = null): AnalyzeReport
fun packDirectory(request: PackDirectoryRequest, progress: ((ProgressEvent) -> Unit)? = null): PackResult
fun preview(request: PreviewRequest, progress: ((ProgressEvent) -> Unit)? = null): PreviewReport
fun createPlan(request: CreatePlanRequest, progress: ((ProgressEvent) -> Unit)? = null): PlanResult
fun applyPlan(request: ApplyPlanRequest, progress: ((ProgressEvent) -> Unit)? = null): ApplyResult
fun verifyInstall(request: VerifyRequest): VerifyResult
fun benchmark(request: BenchmarkRequest, progress: ((ProgressEvent) -> Unit)? = null): BenchmarkReport
fun estimateSavings(request: SavingsRequest): SavingsReport
```

`verifyInstall` and `estimateSavings` take no progress lambda.

### Asynchronous variants

Three operations offer a `CompletableFuture` variant, run on a daemon thread
pool the client owns (and shuts down on `close()`):

```kotlin
fun previewAsync(request: PreviewRequest): CompletableFuture<PreviewReport>
fun createPlanAsync(request: CreatePlanRequest): CompletableFuture<PlanResult>
fun applyPlanAsync(request: ApplyPlanRequest): CompletableFuture<ApplyResult>
```

```kotlin
CavsClient.create().use { cavs ->
    val future = cavs.createPlanAsync(
        CreatePlanRequest(
            oldPath = "Build_v1",
            newPath = "Build_v2",
            outputPlan = "update.cavsplan",
        ),
    )
    val plan = future.join()
    println("Plan is ${plan.planBytes} bytes across ${plan.operationCount} ops")
}
```

### Request and response data classes

```kotlin
@Serializable
data class AnalyzeRequest(
    val oldPath: String,
    val newPath: String,
    val engineHint: String? = null,   // default "auto"
    val maxWorstFiles: Int? = null,   // default 10
)

@Serializable
data class AnalyzeReport(
    val summary: AnalyzeSummary,
    val engine: String = "",
    val warnings: List<String> = emptyList(),
    val recommendations: List<Recommendation> = emptyList(),
    val note: String = "",
)

@Serializable
data class AnalyzeSummary(
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val estimatedUpdateBytes: Long = 0,
    val estimatedSteamPipeBytes: Long = 0,
    val cavsReuseRatio: Double = 0.0,
    val steamPipeReuseRatio: Double = 0.0,
    val filesUnchanged: Int = 0,
    val filesModified: Int = 0,
    val filesAdded: Int = 0,
    val filesDeleted: Int = 0,
    val worstFiles: List<WorstFile> = emptyList(),
)
```

```kotlin
@Serializable
data class PackDirectoryRequest(
    val inputDir: String,
    val outputCavs: String,
    val profile: String? = null,      // default "auto" (fastcdc-64k)
    val compression: String? = null,  // default "zstd-3"; "none" or "zstd-<1..22>"
    val signKeyPath: String? = null,  // 64-hex-char Ed25519 secret key
    val ignore: List<String>? = null,
)

@Serializable
data class PackResult(
    val outputCavs: String,
    val totalSizeBytes: Long = 0,
    val chunkCount: Long = 0,
    val logicalChunks: Long = 0,
    val logicalRawBytes: Long = 0,
    val storedBytes: Long = 0,
    val merkleRoot: String = "",
    val filesPacked: Long = 0,
    val entriesIgnored: Long = 0,
    val signed: Boolean = false,
    val profile: String = "",
    val elapsedMs: Long = 0,
)
```

Valid `profile` labels: `auto`, `fastcdc-16k`, `fastcdc-32k`, `fastcdc-64k`,
`fastcdc-128k`, `fastcdc-256k`, `fixed-256k`, `fixed-512k`, `fixed-1m`.

```kotlin
@Serializable
data class PreviewRequest(
    val oldPath: String,
    val newPath: String,
    val engineHint: String? = null,
    val routes: List<String>? = null, // null/empty = all routes
    val policy: String? = null,        // e.g. "balanced", "networkMin", "hddFriendly"
)

@Serializable
data class Route(
    val name: String,
    val networkBytes: Long = 0,
    val diffMs: Long? = null,
    val applyMs: Long? = null,
    val available: Boolean = true,
)

@Serializable
data class PreviewReport(
    val recommendedRoute: String = "",
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val routes: List<Route> = emptyList(),
    val explanation: String = "",
)
```

```kotlin
@Serializable
data class CreatePlanRequest(
    val newPath: String,
    val outputPlan: String,
    val oldPath: String? = null,       // oldPath OR oldSignature
    val oldSignature: String? = null,
    val planKind: String? = null,      // "portable" (default) or "analysis"
    val blockKib: Int? = null,         // default 64
    val zstdLevel: Int? = null,        // default 19
)

@Serializable
data class PlanResult(
    val planPath: String,
    val planBytes: Long = 0,
    val planKind: String = "",
    val mode: String = "",
    val operationCount: Long = 0,
    val copyOps: Long = 0,
    val inlineOps: Long = 0,
    val reusedBytes: Long = 0,
    val inlineBytes: Long = 0,
    val estimatedNetworkBytes: Long = 0,
    val expectedOutputSize: Long = 0,
    val files: Long = 0,
    val unchangedFiles: Long = 0,
    val deleted: Long = 0,
    val elapsedMs: Long = 0,
)
```

```kotlin
@Serializable
data class ApplyPlanRequest(
    val oldPath: String,
    val planPath: String,
    val outputPath: String,
    val checkOld: Boolean? = null,      // re-hash old source vs plan's BLAKE3
    val deleteRemoved: Boolean? = null, // directory mode
)

@Serializable
data class ApplyResult(
    val outputPath: String,
    val verified: Boolean = false,
    val mode: String = "",
    val filesTotal: Long = 0,
    val filesWritten: Long = 0,
    val filesNoop: Long = 0,
    val dirsCreated: Long = 0,
    val symlinksCreated: Long = 0,
    val deleted: Long = 0,
    val bytesWritten: Long = 0,
    val bytesFromOld: Long = 0,
    val bytesFromBlob: Long = 0,
    val elapsedMs: Long = 0,
)
```

```kotlin
@Serializable
data class VerifyRequest(
    val target: String,
    val signature: String? = null,     // exactly one of signature / manifest
    val manifest: String? = null,
    val allowExtra: Boolean? = null,
)

@Serializable
data class Mismatches(
    val modified: List<String> = emptyList(),
    val missing: List<String> = emptyList(),
    val extra: List<String> = emptyList(),
)

@Serializable
data class VerifyResult(
    val verified: Boolean = false,
    val filesChecked: Long = 0,
    val bytesChecked: Long = 0,
    val mismatches: Mismatches = Mismatches(),
    val elapsedMs: Long = 0,
)
```

```kotlin
@Serializable
data class BenchmarkRequest(
    val oldPath: String,
    val newPath: String,
    val engineHint: String? = null,
    val measureApply: Boolean? = null, // measures plan apply into a temp dir
)

@Serializable
data class BenchmarkReport(
    val oldSizeBytes: Long = 0,
    val newSizeBytes: Long = 0,
    val recommendedRoute: String = "",
    val routes: List<Route> = emptyList(),
    val reuseRatio: Double = 0.0,
)
```

```kotlin
@Serializable
data class SavingsRequest(
    val pricePerGb: Double,
    val monthlyDownloads: Double,
    val averageFullDownloadBytes: Double,
    val averageCavsDownloadBytes: Double,
)

@Serializable
data class SavingsReport(
    val fullDownloadMonthlyCost: Double = 0.0,
    val cavsMonthlyCost: Double = 0.0,
    val estimatedMonthlySavings: Double = 0.0,
    val savingsPercent: Double = 0.0,
)
```

## Progress

Pass a lambda as the second argument. `ProgressEvent` is:

```kotlin
@Serializable
data class ProgressEvent(
    val type: String = "",       // "started", "phaseChanged", "progress", "completed", "failed"
    val operation: String = "",
    val phase: String? = null,
    val currentBytes: Long = 0,
    val totalBytes: Long = 0,
    val percentage: Double? = null,
    val message: String? = null,
)
```

```kotlin
cavs.packDirectory(
    PackDirectoryRequest(inputDir = "Build_v2", outputCavs = "build_v2.cavs"),
) { event ->
    if (event.type == "progress") {
        println("[${event.phase}] ${event.currentBytes}/${event.totalBytes}")
    }
}
```

A malformed progress event is swallowed and never breaks the operation. The
callback is registered for the one call only.

## Error handling

Failures throw `CavsException`:

```kotlin
class CavsException(
    val wireCode: String,                 // raw "CAVS-E-*" string
    message: String,
    val recoverable: Boolean = false,
    val details: Map<String, Any?> = emptyMap(),
) : RuntimeException(message) {
    val code: CavsErrorCode               // parsed enum (UNKNOWN if the SDK predates the code)
}
```

`CavsErrorCode` mirrors the Rust `CAVS-E-*` set: `PATH_NOT_FOUND`,
`PATH_TRAVERSAL`, `INVALID_REQUEST`, `INVALID_JSON`, `UNKNOWN_OPERATION`,
`UNSUPPORTED_SCHEMA`, `CANCELLED`, `PLAN`, `SIGNATURE`, `FORMAT`, `IO`,
`INTERNAL`, and `UNKNOWN` (fallback). Each enum carries its `.wire` string.

```kotlin
import com.cavs.sdk.CavsErrorCode
import com.cavs.sdk.CavsException

try {
    cavs.analyze(AnalyzeRequest(oldPath = "Build_v1", newPath = "Build_v2"))
} catch (e: CavsException) {
    when (e.code) {
        CavsErrorCode.PATH_NOT_FOUND -> println("a build path is missing")
        CavsErrorCode.IO -> if (e.recoverable) retry() else throw e
        else -> throw e
    }
}
```

## Jenkins / Gradle CI example

The Gradle `test` task already wires `--enable-native-access=ALL-UNNAMED` and
passes `CAVS_SDK_LIBRARY` through to the test JVM (as `-Dcavs.sdk.library`).
A Jenkins pipeline that builds the native library and runs the tests:

```groovy
pipeline {
  agent any
  tools { jdk 'temurin-22' }
  environment {
    CAVS_SDK_LIBRARY = "${WORKSPACE}/target/release/libcavs_sdk.so"
  }
  stages {
    stage('Build native') {
      steps { sh 'cargo build --release -p cavs-ffi' }
    }
    stage('Test SDK') {
      steps { dir('sdks/kotlin') { sh 'gradle test' } }
    }
  }
}
```

Maven equivalent (the surefire plugin already sets the JVM args):

```sh
export CAVS_SDK_LIBRARY="$WORKSPACE/target/release/libcavs_sdk.so"
mvn -B verify
```

## Troubleshooting

- **`UnsatisfiedLinkError: native library not bundled … set -Dcavs.sdk.library
  or CAVS_SDK_LIBRARY`.** No bundled resource for this `<os>-<arch>` and no
  override set. Build `cavs-ffi` and export `CAVS_SDK_LIBRARY`.
- **`UnsatisfiedLinkError: native library checksum mismatch`.** The bundled
  `.sha256` sidecar does not match the extracted library — the jar is
  corrupt or mismatched; reinstall the correct artifact.
- **`Illegal native access` / `WARNING: … --enable-native-access`.** Run with
  `--enable-native-access=ALL-UNNAMED`.
- **`UnsupportedClassVersionError` or FFM APIs missing.** You are on a JVM
  older than 22. Use a Java 22+ toolchain.
- **`IllegalStateException: cavs: client is closed`.** A method was called
  after `close()` (or after the enclosing `use { }` block). Create a new
  client.
