# CAVS Go SDK (v1.2.0)

Idiomatic Go bindings for CAVS. The SDK loads the same compiled Rust core the
CAVS CLI uses through a stable C ABI (via cgo) — it does not shell out to the
CLI. See [SDKS.md](SDKS.md) for the shared architecture, envelope, operations
and error model.

- Module: `github.com/orelvis15/cavs-oss/sdks/go`
- Import path: `github.com/orelvis15/cavs-oss/sdks/go/cavs`

## Requirements

- Go 1.21+
- A C toolchain (cgo enabled)
- The native library (`libcavs_sdk.{so,dylib}` / `cavs_sdk.dll`) and its
  header, staged under `cavs/native/`.

## Install

```sh
go get github.com/orelvis15/cavs-oss/sdks/go
```

Because the SDK uses cgo, the native library and header must be present at
build time. The cgo directives link against `native/` relative to the package
and embed an rpath so the built binary finds the library at runtime.

## Native library setup

From the SDK directory (`sdks/go`):

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

## Client lifecycle

```go
func New(opts ...Option) (*Client, error)
func (c *Client) Close() error
```

`New` creates a client backed by a native context. A `Client` is safe for use
from multiple goroutines — calls are serialized so a per-call progress
callback is never clobbered by a concurrent call. Always `Close()` it (once)
to release the native context; calls after close return an error.

Package-level helpers report native metadata:

```go
func Version() string          // native SDK semver
func ABIVersion() string       // native C ABI contract version
func CapabilitiesJSON() string // capability descriptor as JSON
```

## API

Every method takes a `context.Context` (cancellation propagates to the native
job — see below) and a variadic list of `CallOption`. The only call option is
`WithProgress`.

```go
func (c *Client) Analyze(ctx context.Context, req AnalyzeRequest, opts ...CallOption) (*AnalyzeReport, error)
func (c *Client) PackDirectory(ctx context.Context, req PackDirectoryRequest, opts ...CallOption) (*PackResult, error)
func (c *Client) Preview(ctx context.Context, req PreviewRequest, opts ...CallOption) (*PreviewReport, error)
func (c *Client) CreatePlan(ctx context.Context, req CreatePlanRequest, opts ...CallOption) (*PlanResult, error)
func (c *Client) ApplyPlan(ctx context.Context, req ApplyPlanRequest, opts ...CallOption) (*ApplyResult, error)
func (c *Client) VerifyInstall(ctx context.Context, req VerifyRequest, opts ...CallOption) (*VerifyResult, error)
func (c *Client) Benchmark(ctx context.Context, req BenchmarkRequest, opts ...CallOption) (*BenchmarkReport, error)
func (c *Client) EstimateSavings(ctx context.Context, req SavingsRequest, opts ...CallOption) (*SavingsReport, error)
```

### Request and response types

```go
type AnalyzeRequest struct {
	OldPath       string `json:"oldPath"`
	NewPath       string `json:"newPath"`
	EngineHint    string `json:"engineHint,omitempty"`    // default "auto"
	MaxWorstFiles int    `json:"maxWorstFiles,omitempty"` // default 10
}

type AnalyzeReport struct {
	Summary         AnalyzeSummary   `json:"summary"`
	Engine          string           `json:"engine"`
	Warnings        []string         `json:"warnings"`
	Recommendations []Recommendation `json:"recommendations"`
	Note            string           `json:"note"`
}

type AnalyzeSummary struct {
	OldSizeBytes            uint64      `json:"oldSizeBytes"`
	NewSizeBytes            uint64      `json:"newSizeBytes"`
	EstimatedUpdateBytes    uint64      `json:"estimatedUpdateBytes"`
	EstimatedSteamPipeBytes uint64      `json:"estimatedSteamPipeBytes"`
	CavsReuseRatio          float64     `json:"cavsReuseRatio"`
	SteamPipeReuseRatio     float64     `json:"steamPipeReuseRatio"`
	FilesUnchanged          int         `json:"filesUnchanged"`
	FilesModified           int         `json:"filesModified"`
	FilesAdded              int         `json:"filesAdded"`
	FilesDeleted            int         `json:"filesDeleted"`
	WorstFiles              []WorstFile `json:"worstFiles"`
}
```

```go
type PackDirectoryRequest struct {
	InputDir    string   `json:"inputDir"`
	OutputCavs  string   `json:"outputCavs"`
	Profile     string   `json:"profile,omitempty"`     // default "auto" (fastcdc-64k)
	Compression string   `json:"compression,omitempty"` // default "zstd-3"; "none" or "zstd-<1..22>"
	SignKeyPath string   `json:"signKeyPath,omitempty"` // 64-hex-char Ed25519 secret key
	Ignore      []string `json:"ignore,omitempty"`
}

type PackResult struct {
	OutputCavs      string `json:"outputCavs"`
	TotalSizeBytes  uint64 `json:"totalSizeBytes"`
	ChunkCount      uint64 `json:"chunkCount"`
	LogicalChunks   uint64 `json:"logicalChunks"`
	LogicalRawBytes uint64 `json:"logicalRawBytes"`
	StoredBytes     uint64 `json:"storedBytes"`
	MerkleRoot      string `json:"merkleRoot"`
	FilesPacked     uint64 `json:"filesPacked"`
	EntriesIgnored  uint64 `json:"entriesIgnored"`
	Signed          bool   `json:"signed"`
	Profile         string `json:"profile"`
	ElapsedMs       uint64 `json:"elapsedMs"`
}
```

Valid `Profile` labels: `auto`, `fastcdc-16k`, `fastcdc-32k`,
`fastcdc-64k`, `fastcdc-128k`, `fastcdc-256k`, `fixed-256k`, `fixed-512k`,
`fixed-1m`.

```go
type PreviewRequest struct {
	OldPath    string      `json:"oldPath"`
	NewPath    string      `json:"newPath"`
	EngineHint string      `json:"engineHint,omitempty"`
	Routes     []string    `json:"routes,omitempty"` // empty = all routes
	Policy     RoutePolicy `json:"policy,omitempty"`
}

type Route struct {
	Name         string  `json:"name"`
	NetworkBytes uint64  `json:"networkBytes"`
	DiffMs       *uint64 `json:"diffMs,omitempty"`
	ApplyMs      *uint64 `json:"applyMs,omitempty"`
	Available    bool    `json:"available"`
}

type PreviewReport struct {
	RecommendedRoute string  `json:"recommendedRoute"`
	OldSizeBytes     uint64  `json:"oldSizeBytes"`
	NewSizeBytes     uint64  `json:"newSizeBytes"`
	Routes           []Route `json:"routes"`
	Explanation      string  `json:"explanation"`
}
```

`RoutePolicy` constants: `PolicyBalanced`, `PolicyNetworkMin`,
`PolicyHDDFriendly`. `AllRoutes` is a sentinel (a nil `[]string`) meaning
"model every route".

```go
type CreatePlanRequest struct {
	OldPath      string `json:"oldPath,omitempty"`      // oldPath OR oldSignature
	OldSignature string `json:"oldSignature,omitempty"`
	NewPath      string `json:"newPath"`
	OutputPlan   string `json:"outputPlan"`
	PlanKind     string `json:"planKind,omitempty"` // "portable" (default) or "analysis"
	BlockKiB     uint32 `json:"blockKib,omitempty"` // default 64
	ZstdLevel    int    `json:"zstdLevel,omitempty"` // default 19
}

type PlanResult struct {
	PlanPath              string `json:"planPath"`
	PlanBytes             uint64 `json:"planBytes"`
	PlanKind              string `json:"planKind"`
	Mode                  string `json:"mode"`
	OperationCount        uint64 `json:"operationCount"`
	CopyOps               uint64 `json:"copyOps"`
	InlineOps             uint64 `json:"inlineOps"`
	ReusedBytes           uint64 `json:"reusedBytes"`
	InlineBytes           uint64 `json:"inlineBytes"`
	EstimatedNetworkBytes uint64 `json:"estimatedNetworkBytes"`
	ExpectedOutputSize    uint64 `json:"expectedOutputSize"`
	Files                 uint64 `json:"files"`
	UnchangedFiles        uint64 `json:"unchangedFiles"`
	Deleted               uint64 `json:"deleted"`
	ElapsedMs             uint64 `json:"elapsedMs"`
}
```

```go
type ApplyPlanRequest struct {
	OldPath       string `json:"oldPath"`
	PlanPath      string `json:"planPath"`
	OutputPath    string `json:"outputPath"`
	CheckOld      bool   `json:"checkOld,omitempty"`      // re-hash old source vs plan's BLAKE3
	DeleteRemoved bool   `json:"deleteRemoved,omitempty"` // directory mode
}

type ApplyResult struct {
	OutputPath      string `json:"outputPath"`
	Verified        bool   `json:"verified"`
	Mode            string `json:"mode"`
	FilesTotal      uint64 `json:"filesTotal"`
	FilesWritten    uint64 `json:"filesWritten"`
	FilesNoop       uint64 `json:"filesNoop"`
	DirsCreated     uint64 `json:"dirsCreated"`
	SymlinksCreated uint64 `json:"symlinksCreated"`
	Deleted         uint64 `json:"deleted"`
	BytesWritten    uint64 `json:"bytesWritten"`
	BytesFromOld    uint64 `json:"bytesFromOld"`
	BytesFromBlob   uint64 `json:"bytesFromBlob"`
	ElapsedMs       uint64 `json:"elapsedMs"`
}
```

```go
type VerifyRequest struct {
	Target     string `json:"target"`
	Signature  string `json:"signature,omitempty"` // exactly one of Signature / Manifest
	Manifest   string `json:"manifest,omitempty"`
	AllowExtra bool   `json:"allowExtra,omitempty"`
}

type Mismatches struct {
	Modified []string `json:"modified"`
	Missing  []string `json:"missing"`
	Extra    []string `json:"extra"`
}

type VerifyResult struct {
	Verified     bool       `json:"verified"`
	FilesChecked uint64     `json:"filesChecked"`
	BytesChecked uint64     `json:"bytesChecked"`
	Mismatches   Mismatches `json:"mismatches"`
	ElapsedMs    uint64     `json:"elapsedMs"`
}
```

```go
type BenchmarkRequest struct {
	OldPath      string `json:"oldPath"`
	NewPath      string `json:"newPath"`
	EngineHint   string `json:"engineHint,omitempty"`
	MeasureApply bool   `json:"measureApply"` // measures the plan apply into a temp dir
}

type BenchmarkReport struct {
	OldSizeBytes     uint64  `json:"oldSizeBytes"`
	NewSizeBytes     uint64  `json:"newSizeBytes"`
	RecommendedRoute string  `json:"recommendedRoute"`
	Routes           []Route `json:"routes"`
	ReuseRatio       float64 `json:"reuseRatio"`
}
```

```go
type SavingsRequest struct {
	PricePerGB               float64 `json:"pricePerGb"`
	MonthlyDownloads         float64 `json:"monthlyDownloads"`
	AverageFullDownloadBytes float64 `json:"averageFullDownloadBytes"`
	AverageCavsDownloadBytes float64 `json:"averageCavsDownloadBytes"`
}

type SavingsReport struct {
	FullDownloadMonthlyCost float64 `json:"fullDownloadMonthlyCost"`
	CavsMonthlyCost         float64 `json:"cavsMonthlyCost"`
	EstimatedMonthlySavings float64 `json:"estimatedMonthlySavings"`
	SavingsPercent          float64 `json:"savingsPercent"`
}
```

## Context cancellation

Each operation runs on a native background job that the SDK polls to
completion. When the passed `context.Context` is cancelled, the SDK requests
cooperative cancellation of the native job, then still drains it so no native
worker outlives the handle. The call returns `ctx.Err()`:

```go
ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
defer cancel()

plan, err := client.CreatePlan(ctx, cavs.CreatePlanRequest{
	OldPath:    "Build_v1",
	NewPath:    "Build_v2",
	OutputPlan: "update.cavsplan",
})
if errors.Is(err, context.DeadlineExceeded) {
	log.Fatal("plan timed out")
}
```

## Progress with WithProgress

```go
func WithProgress(fn func(ProgressEvent)) CallOption

type ProgressEvent struct {
	Type         string   `json:"type"`       // "started", "phaseChanged", "progress", "completed", "failed"
	Operation    string   `json:"operation"`
	Phase        string   `json:"phase,omitempty"`
	CurrentBytes uint64   `json:"currentBytes,omitempty"`
	TotalBytes   uint64   `json:"totalBytes,omitempty"`
	Percentage   *float64 `json:"percentage,omitempty"`
	Message      string   `json:"message,omitempty"`
}
```

```go
result, err := client.PackDirectory(ctx, cavs.PackDirectoryRequest{
	InputDir:   "Build_v2",
	OutputCavs: "build_v2.cavs",
}, cavs.WithProgress(func(e cavs.ProgressEvent) {
	if e.Type == "progress" {
		fmt.Printf("[%s] %d/%d bytes\n", e.Phase, e.CurrentBytes, e.TotalBytes)
	}
}))
```

The callback may be invoked from a background thread; keep it fast and
thread-safe. It is registered for the duration of the one call only.

## Error handling

Failed operations return a `*cavs.Error`:

```go
type Error struct {
	Code        string         `json:"code"`
	Message     string         `json:"message"`
	Recoverable bool           `json:"recoverable"`
	Details     map[string]any `json:"details,omitempty"`
}
```

Branch on the stable code with `IsCode` instead of string literals:

```go
report, err := client.Analyze(ctx, cavs.AnalyzeRequest{
	OldPath: "Build_v1",
	NewPath: "Build_v2",
})
if err != nil {
	switch {
	case cavs.IsCode(err, cavs.CodePathNotFound):
		log.Fatal("one of the build paths does not exist")
	case cavs.IsCode(err, cavs.CodeCancelled):
		log.Println("cancelled")
	default:
		var ce *cavs.Error
		if errors.As(err, &ce) {
			log.Fatalf("%s: %s (recoverable=%v)", ce.Code, ce.Message, ce.Recoverable)
		}
		log.Fatal(err)
	}
}
```

Exported code constants: `CodePathNotFound`, `CodePathTraversal`,
`CodeInvalidRequest`, `CodeUnknownOperation`, `CodeCancelled`. The full set of
`CAVS-E-*` codes is in [SDKS.md](SDKS.md#error-model); `IsCode` accepts any of
them as a raw string.

## CI/CD example (GitHub Actions)

Gate a pull request on a route benchmark:

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
      # Rust toolchain to build the native cavs-ffi library.
      - uses: dtolnay/rust-toolchain@stable
      - run: make -C sdks/go test
```

`make -C sdks/go test` builds the native library (`make native`) and then
runs `go test ./...`, so the same target works locally and in CI.

## Troubleshooting

- **`cavs_sdk.h: No such file or directory` / linker cannot find `-lcavs_sdk`.**
  The native library and header are not staged. Run `make native` in
  `sdks/go` first.
- **`dlopen`/`image not found` at runtime.** The build embeds an rpath to the
  package's `native/` directory; if you move the binary, keep the library
  reachable, or rebuild after `make native`.
- **`cgo: C compiler not found`.** Install a C toolchain and ensure cgo is
  enabled (`CGO_ENABLED=1`, the default when a C compiler is present).
- **`cavs: client is closed`.** A method was called after `Close()`. Create a
  new client.
- **ABI mismatch.** If operations fail unexpectedly after a native rebuild,
  confirm `cavs.ABIVersion()` reports `1.0.0` and `cavs.CapabilitiesJSON()`
  lists the operation you are calling.
