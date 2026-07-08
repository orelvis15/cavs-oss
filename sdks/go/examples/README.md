# CAVS Go SDK — examples

Runnable examples for the Go SDK. Each is a standalone `main` package.

## Prerequisites

The SDK binds the native Rust core through cgo, so the native library must be
built and staged before running anything:

```sh
cd sdks/go
make native        # builds cavs-ffi (release) and stages the lib + header
```

You need Go 1.21+ and a C toolchain (cgo enabled). Run all commands below from
`sdks/go`.

## Examples

### `quickstart` — the whole lifecycle, zero setup

Generates two synthetic builds (`Build_v1` / `Build_v2`) in a temp directory,
then walks the full update flow: **analyze → preview → createPlan → applyPlan
→ estimateSavings**. Nothing to download, nothing to clean up.

```sh
go run ./examples/quickstart
```

You'll see how much of the new build is reused from the old one, the wire cost
of each delivery route, a `.cavsplan` being written and applied back to
reconstruct the new build, and a rough egress-savings estimate at scale.

### `preview` — one call against your own builds

Runs just the update preview between two directories you already have on disk.

```sh
go run ./examples/preview --old /path/to/Build_v1 --new /path/to/Build_v2
```

## How the sample builds are made

`quickstart` uses the helper in `internal/sample`. It writes a v1 build and a
v2 derived from it with a realistic mix of changes — files that stay identical,
one patched in place, one added, one removed — using large, repetitive payloads
so CAVS' chunk reuse is easy to see. Read `internal/sample/sample.go` if you
want to shape the data differently.
