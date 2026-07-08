# CAVS Node/TypeScript SDK (v1.2.0)

Node.js / TypeScript SDK for CAVS. It loads the same compiled Rust core the
CAVS CLI uses through a stable C ABI (via [koffi](https://koffi.dev)) — it
does not shell out to the CLI. See [SDKS.md](SDKS.md) for the shared
architecture, envelope, operations and error model.

- Package: `@orelvis15/cavs-sdk`

## Install

```sh
npm install @orelvis15/cavs-sdk
```

Released builds ship the native library in per-platform packages, resolved
automatically as optional dependencies:

- `@orelvis15/cavs-sdk-linux-x64`
- `@orelvis15/cavs-sdk-darwin-arm64`
- `@orelvis15/cavs-sdk-win32-x64`
- …and the other supported targets.

## Native library resolution

The native binding resolves the library in this order:

1. `CAVS_SDK_LIBRARY` — an explicit path to the library file (overrides
   everything).
2. The per-platform package `@orelvis15/cavs-sdk-<os>-<arch>` (`os` is
   `linux` / `darwin` / `win32`, `arch` is `x64` / `arm64`).
3. The local `native/` staging directory (source checkout).

For local development against a source checkout:

```sh
npm run native   # builds cavs-ffi (release) and stages the lib into native/
npm test
```

```sh
CAVS_SDK_LIBRARY=/path/to/libcavs_sdk.dylib node app.js   # override
```

## Quickstart

```ts
import { CavsClient } from "@orelvis15/cavs-sdk";

const cavs = new CavsClient();
try {
  const preview = await cavs.preview({
    oldPath: "Build_v1",
    newPath: "Build_v2",
    policy: "balanced",
  });
  console.log(preview.recommendedRoute);
} finally {
  cavs.close();
}
```

## Client lifecycle

```ts
class CavsClient {
  constructor();
  close(): void;
}
```

A `CavsClient` owns a native context. Calls are serialized onto that one
context (a single native context carries one progress-callback slot), so
concurrent calls on one client run one at a time — **create multiple clients
for parallelism**. Always `close()` it when done. A call after `close()`
rejects with a `CavsError`.

Module-level metadata helpers:

```ts
import { version, abiVersion } from "@orelvis15/cavs-sdk";

version();    // native SDK version
abiVersion(); // native C ABI contract version
```

## API

Every method returns a `Promise` and accepts an optional `CallOptions`:

```ts
analyze(req: AnalyzeRequest, opts?: CallOptions): Promise<AnalyzeReport>;
packDirectory(req: PackDirectoryRequest, opts?: CallOptions): Promise<PackResult>;
preview(req: PreviewRequest, opts?: CallOptions): Promise<PreviewReport>;
createPlan(req: CreatePlanRequest, opts?: CallOptions): Promise<PlanResult>;
applyPlan(req: ApplyPlanRequest, opts?: CallOptions): Promise<ApplyResult>;
verifyInstall(req: VerifyRequest, opts?: CallOptions): Promise<VerifyResult>;
benchmark(req: BenchmarkRequest, opts?: CallOptions): Promise<BenchmarkReport>;
estimateSavings(req: SavingsRequest, opts?: CallOptions): Promise<SavingsReport>;
```

```ts
interface CallOptions {
  onProgress?: (event: ProgressEvent) => void;
  signal?: AbortSignal;
}
```

### Request and response interfaces

```ts
interface AnalyzeRequest {
  oldPath: string;
  newPath: string;
  engineHint?: string;   // default "auto"
  maxWorstFiles?: number; // default 10
}

interface AnalyzeReport {
  summary: {
    oldSizeBytes: number;
    newSizeBytes: number;
    estimatedUpdateBytes: number;
    estimatedSteamPipeBytes: number;
    cavsReuseRatio: number;
    steamPipeReuseRatio: number;
    filesUnchanged: number;
    filesModified: number;
    filesAdded: number;
    filesDeleted: number;
    worstFiles: WorstFile[];
  };
  engine: string;
  warnings: string[];
  recommendations: Recommendation[];
  note: string;
}
```

```ts
interface PackDirectoryRequest {
  inputDir: string;
  outputCavs: string;
  profile?: string;      // default "auto" (fastcdc-64k)
  compression?: string;  // default "zstd-3"; "none" or "zstd-<1..22>"
  signKeyPath?: string;  // 64-hex-char Ed25519 secret key
  ignore?: string[];
}

interface PackResult {
  outputCavs: string;
  totalSizeBytes: number;
  chunkCount: number;
  logicalChunks: number;
  logicalRawBytes: number;
  storedBytes: number;
  merkleRoot: string;
  filesPacked: number;
  entriesIgnored: number;
  signed: boolean;
  profile: string;
  elapsedMs: number;
}
```

Valid `profile` labels: `auto`, `fastcdc-64k`, `fastcdc-128k`, `fastcdc-256k`,
`fixed-256k`, `fixed-512k`, `fixed-1m`.

```ts
interface PreviewRequest {
  oldPath: string;
  newPath: string;
  engineHint?: string;
  routes?: string[]; // empty/omitted = all routes
  policy?: RoutePolicy;
}

type RoutePolicy =
  | "balanced" | "networkMin" | "cpuMin" | "ramMin"
  | "diskIoMin" | "hddFriendly" | "developerFast";

interface Route {
  name: string;
  networkBytes: number;
  diffMs?: number;
  applyMs?: number | null;
  available: boolean;
}

interface PreviewReport {
  recommendedRoute: string;
  oldSizeBytes: number;
  newSizeBytes: number;
  routes: Route[];
  explanation: string;
}
```

```ts
interface CreatePlanRequest {
  oldPath?: string;       // oldPath OR oldSignature
  oldSignature?: string;
  newPath: string;
  outputPlan: string;
  planKind?: "portable" | "analysis"; // default "portable"
  blockKib?: number;      // default 64
  zstdLevel?: number;     // default 19
}

interface PlanResult {
  planPath: string;
  planBytes: number;
  planKind: string;
  mode: string;
  operationCount: number;
  copyOps: number;
  inlineOps: number;
  reusedBytes: number;
  inlineBytes: number;
  estimatedNetworkBytes: number;
  expectedOutputSize: number;
  files: number;
  unchangedFiles: number;
  deleted: number;
  elapsedMs: number;
}
```

```ts
interface ApplyPlanRequest {
  oldPath: string;
  planPath: string;
  outputPath: string;
  checkOld?: boolean;      // re-hash old source vs plan's BLAKE3
  deleteRemoved?: boolean; // directory mode
}

interface ApplyResult {
  outputPath: string;
  verified: boolean;
  mode: string;
  filesTotal: number;
  filesWritten: number;
  filesNoop: number;
  dirsCreated: number;
  symlinksCreated: number;
  deleted: number;
  bytesWritten: number;
  bytesFromOld: number;
  bytesFromBlob: number;
  elapsedMs: number;
}
```

```ts
interface VerifyRequest {
  target: string;
  signature?: string; // exactly one of signature / manifest
  manifest?: string;
  allowExtra?: boolean;
}

interface VerifyResult {
  verified: boolean;
  filesChecked: number;
  bytesChecked: number;
  mismatches: { modified: string[]; missing: string[]; extra: string[] };
  elapsedMs: number;
}
```

```ts
interface BenchmarkRequest {
  oldPath: string;
  newPath: string;
  engineHint?: string;
  measureApply?: boolean; // measures plan apply into a temp dir
}

interface BenchmarkReport {
  oldSizeBytes: number;
  newSizeBytes: number;
  recommendedRoute: string;
  routes: Route[];
  reuseRatio: number;
}
```

```ts
interface SavingsRequest {
  pricePerGb: number;
  monthlyDownloads: number;
  averageFullDownloadBytes: number;
  averageCavsDownloadBytes: number;
}

interface SavingsReport {
  fullDownloadMonthlyCost: number;
  cavsMonthlyCost: number;
  estimatedMonthlySavings: number;
  savingsPercent: number;
}
```

## Cancellation with AbortSignal

Pass an `AbortSignal` to cancel a running operation. Cancellation applies to
the **non-progress path**, which runs the native job off the event loop and
polls for completion:

```ts
const controller = new AbortController();
const timer = setTimeout(() => controller.abort(), 30_000);
try {
  const plan = await cavs.createPlan(
    { oldPath: "Build_v1", newPath: "Build_v2", outputPlan: "update.cavsplan" },
    { signal: controller.signal },
  );
} catch (err) {
  if (err instanceof CavsError && err.code === ErrorCode.Cancelled) {
    console.error("cancelled");
  }
} finally {
  clearTimeout(timer);
}
```

If the signal is already aborted at call time, the promise rejects
immediately with `CAVS-E-CANCELLED`.

## Progress with onProgress

```ts
interface ProgressEvent {
  type: string;         // "started", "phaseChanged", "progress", "completed", "failed"
  operation: string;
  phase?: string;
  currentBytes?: number;
  totalBytes?: number;
  percentage?: number;
  message?: string;
}
```

```ts
await cavs.packDirectory(
  { inputDir: "Build_v2", outputCavs: "build_v2.cavs" },
  {
    onProgress: (e) => {
      if (e.type === "progress") {
        console.log(`[${e.phase}] ${e.currentBytes}/${e.totalBytes}`);
      }
    },
  },
);
```

**Important:** enabling `onProgress` runs the operation **synchronously** on
the calling thread — koffi can only invoke JS callbacks on the thread that
made the native call. This blocks the event loop for the duration of the
operation, so use `onProgress` for CLIs and scripts, not hot request paths.
The non-progress path (no `onProgress`) runs off the event loop and is the one
that supports `signal`. A malformed progress event is swallowed and never
breaks the operation.

## Error handling

Failed operations reject with a `CavsError`:

```ts
class CavsError extends Error {
  readonly code: string;                     // stable "CAVS-E-*"
  readonly recoverable: boolean;
  readonly details: Record<string, unknown>;
}
```

Well-known codes are exported as `ErrorCode` (the engine defines the full
set):

```ts
import { CavsClient, CavsError, ErrorCode } from "@orelvis15/cavs-sdk";

try {
  await cavs.analyze({ oldPath: "Build_v1", newPath: "Build_v2" });
} catch (err) {
  if (err instanceof CavsError) {
    switch (err.code) {
      case ErrorCode.PathNotFound:
        console.error("a build path is missing");
        break;
      case ErrorCode.Cancelled:
        console.error("cancelled");
        break;
      default:
        console.error(`${err.code}: ${err.message} (recoverable=${err.recoverable})`);
    }
  } else {
    throw err;
  }
}
```

`ErrorCode` members: `PathNotFound`, `PathTraversal`, `InvalidRequest`,
`UnknownOperation`, `Cancelled`. The complete `CAVS-E-*` table is in
[SDKS.md](SDKS.md#error-model); `err.code` is always the raw wire string, so
you can compare against any of them.

## CI example

```ts
import { CavsClient } from "@orelvis15/cavs-sdk";

const cavs = new CavsClient();
try {
  const report = await cavs.benchmark({
    oldPath: process.env.OLD_BUILD!,
    newPath: process.env.NEW_BUILD!,
  });
  const cavsPlan = report.routes.find((r) => r.name === "cavsPlan");
  console.log(`cavsPlan: ${cavsPlan?.networkBytes} bytes, diff ${cavsPlan?.diffMs}ms`);
  // Gate the pipeline on a threshold, upload the report, etc.
} finally {
  cavs.close();
}
```

A minimal GitHub Actions job:

```yaml
name: CAVS Benchmark
on: [push]
jobs:
  bench:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: lts/* }
      - run: npm ci
      - run: node scripts/cavs-benchmark.mjs
```

## Troubleshooting

- **`cavs: native library not found (looked for …)`.** No `CAVS_SDK_LIBRARY`,
  no matching per-platform package, and nothing staged in `native/`. Install
  the platform package, set `CAVS_SDK_LIBRARY`, or run `npm run native`.
- **The event loop stalls during an operation.** You passed `onProgress`,
  which forces the synchronous path. Drop it (or move the work to a worker
  thread) if you need the loop free.
- **`AbortSignal` has no effect.** Cancellation only applies to the
  non-progress path; it is ignored when `onProgress` is set.
- **`client is closed` rejection.** A method was called after `close()`.
  Create a new client.
- **koffi load errors (`ERR_DLOPEN_FAILED`, wrong architecture).** The
  resolved library does not match the running Node ABI/arch. Confirm the
  per-platform package matches `process.platform`/`process.arch`, or point
  `CAVS_SDK_LIBRARY` at a correct build.
