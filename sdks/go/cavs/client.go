package cavs

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
	"time"
)

// schemaVersion is the request envelope version this SDK speaks.
const schemaVersion = "1.0"

// pollInterval is how often a running native job is polled for completion.
var pollInterval = 500 * time.Microsecond

// Client is a CAVS SDK client. It owns a native context and is safe for
// use from multiple goroutines; calls are serialized so a per-call progress
// callback is not clobbered by a concurrent call.
type Client struct {
	native *nativeContext
	mu     sync.Mutex
	closed bool
}

// Option configures a Client.
type Option func(*clientConfig)

type clientConfig struct{}

// New creates a Client backed by the native library.
func New(opts ...Option) (*Client, error) {
	var cfg clientConfig
	for _, o := range opts {
		o(&cfg)
	}
	n := newNativeContext()
	if n.ptr == nil {
		return nil, errors.New("cavs: failed to create native context")
	}
	return &Client{native: n}, nil
}

// Close releases the native context. Further calls return an error.
func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil
	}
	c.closed = true
	c.native.free()
	return nil
}

// callOptions carries per-call settings (progress sink).
type callOptions struct {
	progress func(ProgressEvent)
}

// CallOption customizes a single operation call.
type CallOption func(*callOptions)

// WithProgress registers a progress callback for one call.
func WithProgress(fn func(ProgressEvent)) CallOption {
	return func(o *callOptions) { o.progress = fn }
}

// envelope is the request wrapper the engine expects.
type envelope struct {
	SchemaVersion string `json:"schemaVersion"`
	Data          any    `json:"data"`
}

// response is the engine's reply envelope.
type response struct {
	OK    bool            `json:"ok"`
	Error *Error          `json:"error"`
	Data  json.RawMessage `json:"data"`
}

// execute runs one operation, honoring ctx cancellation, and unmarshals the
// result data into out.
func (c *Client) execute(ctx context.Context, operation string, req any, out any, opts []CallOption) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return errors.New("cavs: client is closed")
	}
	if err := ctx.Err(); err != nil {
		return err
	}

	var co callOptions
	for _, o := range opts {
		o(&co)
	}
	c.native.setProgress(co.progress)
	defer c.native.setProgress(nil)

	body, err := json.Marshal(envelope{SchemaVersion: schemaVersion, Data: req})
	if err != nil {
		return err
	}

	j := c.native.start(operation, string(body))
	if j == nil {
		return &Error{Code: CodeInvalidRequest, Message: "native library rejected the request"}
	}
	defer j.free()

	envJSON, err := c.await(ctx, j)
	if err != nil {
		return err
	}

	var resp response
	if err := json.Unmarshal([]byte(envJSON), &resp); err != nil {
		return err
	}
	if !resp.OK {
		if resp.Error != nil {
			return resp.Error
		}
		return &Error{Code: "CAVS-E-INTERNAL", Message: "operation failed without an error body"}
	}
	if out != nil && len(resp.Data) > 0 {
		return json.Unmarshal(resp.Data, out)
	}
	return nil
}

// await polls the job until completion, cancelling the native job if ctx is
// cancelled. It still drains the job to completion after cancellation so no
// native worker outlives the handle.
func (c *Client) await(ctx context.Context, j *job) (string, error) {
	cancelled := false
	for {
		if env, done := j.poll(); done {
			if cancelled {
				return "", ctx.Err()
			}
			return env, nil
		}
		if !cancelled {
			select {
			case <-ctx.Done():
				j.cancel()
				cancelled = true
			default:
			}
		}
		time.Sleep(pollInterval)
	}
}

// Analyze inspects an old→new build transition.
func (c *Client) Analyze(ctx context.Context, req AnalyzeRequest, opts ...CallOption) (*AnalyzeReport, error) {
	var out AnalyzeReport
	return &out, c.execute(ctx, "analyze", req, &out, opts)
}

// PackDirectory packages a directory tree into a .cavs container.
func (c *Client) PackDirectory(ctx context.Context, req PackDirectoryRequest, opts ...CallOption) (*PackResult, error) {
	var out PackResult
	return &out, c.execute(ctx, "packDirectory", req, &out, opts)
}

// Preview estimates the wire cost of an update across delivery routes.
func (c *Client) Preview(ctx context.Context, req PreviewRequest, opts ...CallOption) (*PreviewReport, error) {
	var out PreviewReport
	return &out, c.execute(ctx, "previewUpdate", req, &out, opts)
}

// CreatePlan builds a portable .cavsplan.
func (c *Client) CreatePlan(ctx context.Context, req CreatePlanRequest, opts ...CallOption) (*PlanResult, error) {
	var out PlanResult
	return &out, c.execute(ctx, "createPlan", req, &out, opts)
}

// ApplyPlan applies a .cavsplan to an old build.
func (c *Client) ApplyPlan(ctx context.Context, req ApplyPlanRequest, opts ...CallOption) (*ApplyResult, error) {
	var out ApplyResult
	return &out, c.execute(ctx, "applyPlan", req, &out, opts)
}

// VerifyInstall checks an installed build against a signature or manifest.
func (c *Client) VerifyInstall(ctx context.Context, req VerifyRequest, opts ...CallOption) (*VerifyResult, error) {
	var out VerifyResult
	return &out, c.execute(ctx, "verifyInstall", req, &out, opts)
}

// Benchmark produces a route-comparison report.
func (c *Client) Benchmark(ctx context.Context, req BenchmarkRequest, opts ...CallOption) (*BenchmarkReport, error) {
	var out BenchmarkReport
	return &out, c.execute(ctx, "benchmark", req, &out, opts)
}

// EstimateSavings computes monthly egress savings from a pricing model.
func (c *Client) EstimateSavings(ctx context.Context, req SavingsRequest, opts ...CallOption) (*SavingsReport, error) {
	var out SavingsReport
	return &out, c.execute(ctx, "estimateSavings", req, &out, opts)
}
