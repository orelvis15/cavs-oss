# @orelvis15/cavs-sdk

Node.js / TypeScript SDK for CAVS. It loads the same compiled Rust core the
CAVS CLI uses through a stable C ABI (via [koffi](https://koffi.dev)) — it
does not shell out to the CLI.

## Install

```sh
npm install @orelvis15/cavs-sdk
```

Released builds ship the native library in per-platform packages
(`@orelvis15/cavs-sdk-linux-x64`, `@orelvis15/cavs-sdk-darwin-arm64`, `@orelvis15/cavs-sdk-win32-x64`, …),
resolved automatically. For local development against a source checkout:

```sh
npm run native   # builds cavs-ffi (release) and stages the lib into native/
npm test
```

`CAVS_SDK_LIBRARY=/path/to/libcavs_sdk.dylib` overrides library resolution.

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

## API

`CavsClient` exposes `analyze`, `packDirectory`, `preview`, `createPlan`,
`applyPlan`, `verifyInstall`, `benchmark` and `estimateSavings`. Every method
returns a `Promise` and accepts an options object:

- `onProgress(event)` — stream `ProgressEvent`s. Progress runs the operation
  synchronously (koffi can only invoke JS callbacks on the calling thread),
  so use it for CLIs/scripts rather than hot request paths.
- `signal` — an `AbortSignal` that cancels the native job (non-progress
  path, which runs off the event loop and polls for completion).

Failures reject with a `CavsError` carrying a stable `.code` (see
`ErrorCode`).

## Examples

Runnable examples live in [`examples/`](examples/). `examples/endToEnd`
generates two synthetic builds and walks the full lifecycle (analyze → preview
→ createPlan → applyPlan → estimateSavings) with zero setup:

```sh
npm run build
node dist/examples/endToEnd.js
```

See [`examples/README.md`](examples/README.md) for the full list.

## CI example

```ts
const preview = await cavs.preview({
  oldPath: process.env.OLD_BUILD!,
  newPath: process.env.NEW_BUILD!,
  routes: [],
});
await uploadReport(preview);
```
