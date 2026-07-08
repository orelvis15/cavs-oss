# CAVS SDKs (v1.2.0)

The CAVS SDKs let you drive the CAVS engine — analyze, pack, plan, apply,
verify, benchmark builds and estimate egress savings — from Go, Kotlin/JVM
and Node.js/TypeScript, **without shelling out to the CLI**. Every SDK loads
the same compiled Rust core the `cavs` CLI uses, through a small, stable C
ABI. The heavy lifting (chunking, diffing, signatures, apply) runs natively;
the language bindings are thin.

- **Go** — [SDK_GO.md](SDK_GO.md)
- **Kotlin/JVM** — [SDK_KOTLIN.md](SDK_KOTLIN.md)
- **Node.js / TypeScript** — [SDK_NODE.md](SDK_NODE.md)
- **C ABI reference** (for authoring a new binding) — [SDK_NATIVE_ABI.md](SDK_NATIVE_ABI.md)

## Architecture

Every SDK is one layer of a stack that funnels through a single, coarse
JSON-in / JSON-out boundary:

```text
┌──────────────────────────────────────────────────────────────┐
│  Go bindings      Kotlin bindings      Node bindings           │
│  (cgo)            (FFM / JEP 454)       (koffi)                 │
└───────────────┬───────────────┬───────────────┬───────────────┘
                │               │               │
                ▼               ▼               ▼
        ┌───────────────────────────────────────────────┐
        │  cavs-ffi — stable C ABI (cavs_sdk.h)           │
        │  opaque handles, JSON in / JSON out, jobs       │
        └───────────────────────┬───────────────────────┘
                                ▼
        ┌───────────────────────────────────────────────┐
        │  cavs-sdk-core — JSON operation engine          │
        │  envelope parse, dispatch, error mapping        │
        └───────────────────────┬───────────────────────┘
                                ▼
        ┌───────────────────────────────────────────────┐
        │  Rust core crates — cavs-analyzer, cavs-plan,   │
        │  cavs-signature, cavs-format, cavs-chunker, …   │
        └───────────────────────────────────────────────┘
```

The boundary is JSON on purpose: CAVS operations are file-system and
compression heavy, so JSON overhead at the FFI edge is negligible, and the
ABI stays stable while the Rust internals evolve freely.

## The JSON envelope

Each operation is invoked with an operation name plus a request envelope, and
returns a response envelope. The SDKs build and parse these for you — you
only ever see typed request/response structs — but the shapes are worth
knowing.

**Request:**

```json
{ "schemaVersion": "1.0", "requestId": "optional", "data": { ... } }
```

- `schemaVersion` — the schema the request speaks. Only major `1` is
  accepted; any other major is rejected with `CAVS-E-UNSUPPORTED-SCHEMA`.
  It may be omitted (a bare object is accepted as a convenience).
- `requestId` — optional; echoed back verbatim on the response.
- `data` — the operation-specific payload (see the operations table below).

**Success response:**

```json
{ "schemaVersion": "1.0", "ok": true, "operation": "previewUpdate",
  "requestId": "optional", "data": { ... } }
```

**Error response:**

```json
{ "schemaVersion": "1.0", "ok": false, "operation": "analyze",
  "error": { "code": "CAVS-E-PATH-NOT-FOUND",
             "message": "/no/such/path does not exist",
             "recoverable": false, "details": {} } }
```

The current schema version is `1.0`; the ABI contract version is `1.0.0`.

## Operations

The engine understands eight operations. `previewUpdate` and `compareRoutes`
are aliases for the same implementation. All request/response field names are
camelCase.

| Operation | What it does | Key request fields | Key response fields |
|---|---|---|---|
| `analyze` | Inspect an old→new build transition: sizes, per-file cost, findings and recommendations. | `oldPath`, `newPath`, `engineHint` (default `auto`), `maxWorstFiles` (default 10) | `summary` (sizes, `estimatedUpdateBytes`, `cavsReuseRatio`, file counts, `worstFiles`), `engine`, `warnings`, `recommendations`, `note` |
| `packDirectory` | Package a directory tree as a deduplicated `.cavs` container (per-file SHA-256, `.cavsignore`, optional signing). | `inputDir`, `outputCavs`, `profile` (default `auto`→`fastcdc-64k`), `compression` (default `zstd-3`), `signKeyPath`, `ignore` | `outputCavs`, `totalSizeBytes`, `chunkCount`, `logicalChunks`, `storedBytes`, `merkleRoot`, `filesPacked`, `entriesIgnored`, `signed`, `profile`, `elapsedMs` |
| `previewUpdate` (alias `compareRoutes`) | Estimate wire cost of shipping `newPath` to a client that has `oldPath`, across delivery routes; recommend the cheapest. | `oldPath`, `newPath`, `engineHint`, `routes` (empty = all) | `recommendedRoute`, `oldSizeBytes`, `newSizeBytes`, `routes[]` (`name`, `networkBytes`, `available`), `explanation` |
| `createPlan` | Build a portable `.cavsplan` from an old build (or `.cavssig`) and a new build. | `newPath`, `outputPlan`, `oldPath` **or** `oldSignature`, `planKind` (`portable`/`analysis`), `blockKib` (default 64), `zstdLevel` (default 19) | `planPath`, `planBytes`, `planKind`, `mode`, `operationCount`, `copyOps`, `inlineOps`, `reusedBytes`, `inlineBytes`, `expectedOutputSize`, `files`, `elapsedMs` |
| `applyPlan` | Apply a `.cavsplan` to an old build, producing the new build (atomic artifact / journaled directory apply). | `oldPath`, `planPath`, `outputPath`, `checkOld`, `deleteRemoved` | `outputPath`, `verified`, `mode`, `filesTotal`, `filesWritten`, `filesNoop`, `dirsCreated`, `symlinksCreated`, `deleted`, `bytesWritten`, `bytesFromOld`, `bytesFromBlob`, `elapsedMs` |
| `verifyInstall` | Check an installed build against a known-good `.cavssig` **or** a manifest's recorded SHA-256 digests. | `target`, exactly one of `signature` / `manifest`, `allowExtra` | `verified`, `filesChecked`, `bytesChecked`, `mismatches` (`modified`, `missing`, `extra`), `elapsedMs` |
| `benchmark` | Repeatable route-comparison report for CI/CD; adds measured diff/apply timings for the CAVS plan route. | `oldPath`, `newPath`, `engineHint`, `measureApply` (default `true`) | `oldSizeBytes`, `newSizeBytes`, `recommendedRoute`, `routes[]` (with `diffMs`/`applyMs` on the `cavsPlan` route), `reuseRatio` |
| `estimateSavings` | Pure arithmetic over a pricing model: monthly egress cost of full downloads vs CAVS updates. | `pricePerGb`, `monthlyDownloads`, `averageFullDownloadBytes`, `averageCavsDownloadBytes` | `fullDownloadMonthlyCost`, `cavsMonthlyCost`, `estimatedMonthlySavings`, `savingsPercent` |

The routes modeled by `previewUpdate`/`benchmark` are `fullRaw`,
`steamPipeStyle`, `cavsChunk` and `cavsPlan` (`cavsPlan` is the exact encoded
size of a portable `.cavsplan`).

## Error model

Every failure maps to a stable `CAVS-E-*` code, so bindings can surface typed
errors without parsing prose. The error object carries `code`, `message`, a
`recoverable` flag (whether retrying the same request could transiently
succeed) and optional `details`.

| Code | Meaning | Recoverable |
|---|---|---|
| `CAVS-E-INVALID-REQUEST` | Request payload was malformed or failed validation. | no |
| `CAVS-E-INVALID-JSON` | Request JSON did not parse. | no |
| `CAVS-E-UNKNOWN-OPERATION` | Operation name is not one of the eight. | no |
| `CAVS-E-UNSUPPORTED-SCHEMA` | `schemaVersion` major is not `1`. | no |
| `CAVS-E-PATH-NOT-FOUND` | An input path (build, plan, signature) does not exist. | no |
| `CAVS-E-PATH-TRAVERSAL` | A tree entry resolved to an unsafe path. | no |
| `CAVS-E-CANCELLED` | The operation was cancelled cooperatively. | yes |
| `CAVS-E-PLAN` | An error from the plan engine (`cavs-plan`). | no |
| `CAVS-E-SIGNATURE` | An error from the signature engine (`cavs-signature`). | no |
| `CAVS-E-FORMAT` | An error from the container format layer (`cavs-format`). | no |
| `CAVS-E-IO` | An underlying I/O error. | yes |
| `CAVS-E-INTERNAL` | An unexpected internal error. | no |

Only `CAVS-E-IO` and `CAVS-E-CANCELLED` are marked `recoverable`.

## Capability discovery

Each SDK exposes the native version, the ABI version and a capability
descriptor. The descriptor is the source of truth for what a given native
library supports:

```json
{
  "abiVersion": "1.0.0",
  "sdkVersion": "1.2.0",
  "schemaVersion": "1.0",
  "features": ["analyze", "packDirectory", "previewUpdate", "compareRoutes",
               "createPlan", "applyPlan", "verifyInstall", "benchmark",
               "estimateSavings"],
  "platform": { "os": "linux", "arch": "x86_64" }
}
```

- Go — `cavs.Version()`, `cavs.ABIVersion()`, `cavs.CapabilitiesJSON()`
- Kotlin — `client.version()`, `client.abiVersion()` (and the bridge's `capabilitiesJson()`)
- Node — `version()`, `abiVersion()` (and `native.capabilitiesJson()`)

## SDK comparison

| | Go | Kotlin/JVM | Node/TypeScript |
|---|---|---|---|
| Package | `github.com/orelvis15/cavs-oss/sdks/go` | `io.github.orelvis15:cavs-sdk` | `@orelvis15/cavs-sdk` |
| Install | `go get github.com/orelvis15/cavs-oss/sdks/go` | Gradle/Maven dependency | `npm install @orelvis15/cavs-sdk` |
| Bridge | cgo | Java FFM (JEP 454) | koffi |
| Runtime requirement | Go 1.21+, C toolchain | Java 22+ | Node.js |
| Async model | Synchronous methods driven by a background native job; `context.Context` propagates cancellation | Synchronous methods + `*Async` variants returning `CompletableFuture` | `Promise`-returning methods |
| Cancellation | `context.Context` | (interrupt the future / thread) | `AbortSignal` (non-progress path) |
| Progress | `cavs.WithProgress(fn)` per call | `(ProgressEvent) -> Unit` lambda per call | `onProgress` in call options (runs sync) |
| Error type | `*cavs.Error` (`Code`, `IsCode`) | `CavsException` (`CavsErrorCode`) | `CavsError` (`code`, `ErrorCode`) |

## Native library

The SDKs bind to one shared native library. Names by platform:

| Platform | Library file |
|---|---|
| Linux / BSD | `libcavs_sdk.so` |
| macOS | `libcavs_sdk.dylib` |
| Windows | `cavs_sdk.dll` |

Supported native targets: `linux-x86_64`, `linux-aarch64`, `macos-x86_64`,
`macos-aarch64`, `windows-x86_64`.

Released packages bundle or resolve the correct library automatically. For a
source checkout, build it from the workspace and point the SDK at it with the
`CAVS_SDK_LIBRARY` environment variable — see each SDK's doc for specifics.
