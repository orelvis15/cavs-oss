# CAVS Go SDK

Idiomatic Go bindings for CAVS. The SDK loads the same compiled Rust core
the CAVS CLI uses through a stable C ABI (via cgo) — it does not shell out
to the CLI.

## Requirements

- Go 1.21+
- A C toolchain (cgo enabled)
- The native library `libcavs_sdk.{so,dylib}` / `cavs_sdk.dll` and header,
  staged under `cavs/native/`.

## Building the native library

From this directory:

```sh
make native   # builds cavs-ffi (release) and stages the lib + header
make test     # make native, then `go test ./...`
```

`make native` compiles `core/cavs-ffi` in the Rust workspace and copies the
platform library plus `cavs_sdk.h` into `cavs/native/`.

## Quickstart

```go
package main

import (
	"context"
	"fmt"

	"github.com/orelvis15/cavs-oss/sdks/go/cavs"
)

func main() {
	client, err := cavs.New()
	if err != nil {
		panic(err)
	}
	defer client.Close()

	preview, err := client.Preview(context.Background(), cavs.PreviewRequest{
		OldPath: "Build_v1",
		NewPath: "Build_v2",
		Policy:  cavs.PolicyBalanced,
	})
	if err != nil {
		panic(err)
	}
	fmt.Println("Recommended route:", preview.RecommendedRoute)
}
```

## API

`Client` exposes `Analyze`, `PackDirectory`, `Preview`, `CreatePlan`,
`ApplyPlan`, `VerifyInstall`, `Benchmark` and `EstimateSavings`. Every
method takes a `context.Context` (cancellation propagates to the native
job) and accepts `cavs.WithProgress(fn)` to stream `ProgressEvent`s.

Errors are `*cavs.Error` carrying a stable `Code` (e.g.
`cavs.CodePathNotFound`); use `cavs.IsCode(err, code)` to branch.

## Examples

Runnable examples live in [`examples/`](examples/). `examples/quickstart`
generates two synthetic builds and walks the full lifecycle (analyze → preview
→ createPlan → applyPlan → estimateSavings) with zero setup:

```sh
make native
go run ./examples/quickstart
```

See [`examples/README.md`](examples/README.md) for the full list.

## CI/CD example

```yaml
name: CAVS Preview
on: [push]
jobs:
  preview:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-go@v5
        with: { go-version: stable }
      - run: make -C sdks/go test
```
