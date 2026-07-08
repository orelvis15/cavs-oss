# CAVS Node SDK — examples

Runnable examples for the Node / TypeScript SDK.

## Prerequisites

Install and compile the SDK. For a source checkout you also need the native
library staged locally:

```sh
cd sdks/node
npm install
npm run native     # local dev only: builds cavs-ffi (release) and stages the lib
npm run build      # compiles src/ and examples/ into dist/
```

If you installed the published package (`npm install @orelvis15/cavs-sdk`), the
native library ships in a per-platform package and `npm run native` is not
needed. Run all commands below from `sdks/node`.

> The examples are written in TypeScript and compiled to `dist/examples/`.
> Run the compiled `.js` files with `node`.

## Examples

### `endToEnd` — the whole lifecycle, zero setup

Generates two synthetic builds in a temp directory, then walks the full update
flow: **analyze → preview → createPlan → applyPlan → estimateSavings**. Nothing
to download, nothing to clean up.

```sh
node dist/examples/endToEnd.js
```

You'll see how much of the new build is reused from the old one, the wire cost
of each delivery route, a `.cavsplan` being written and applied back to
reconstruct the new build, and a rough egress-savings estimate at scale.

### `preview` — one call against your own builds

Runs just the update preview between two directories you already have on disk.

```sh
node dist/examples/preview.js --old /path/to/Build_v1 --new /path/to/Build_v2
```

## How the sample builds are made

`endToEnd` uses the helper in `sample.ts`. It writes a v1 build and a v2 derived
from it with a realistic mix of changes — files that stay identical, one patched
in place, one added, one removed — using large, repetitive payloads so CAVS'
chunk reuse is easy to see. Edit `sample.ts` if you want to shape the data
differently.
