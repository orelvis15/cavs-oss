package cavs

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func newClient(t *testing.T) *Client {
	t.Helper()
	c, err := New()
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	t.Cleanup(func() { _ = c.Close() })
	return c
}

// makeBuilds writes a deterministic old/new pair sharing most bytes.
func makeBuilds(t *testing.T) (string, string) {
	t.Helper()
	root := t.TempDir()
	old := filepath.Join(root, "Build_v1")
	newDir := filepath.Join(root, "Build_v2")
	mustMkdir(t, filepath.Join(old, "data"))
	mustMkdir(t, filepath.Join(newDir, "data"))

	base := make([]byte, 512*1024)
	for i := range base {
		base[i] = byte(i % 251)
	}
	mustWrite(t, filepath.Join(old, "data/asset.bin"), base)
	changed := make([]byte, len(base))
	copy(changed, base)
	for i := 300000; i < 304096; i++ {
		changed[i] ^= 0xFF
	}
	mustWrite(t, filepath.Join(newDir, "data/asset.bin"), changed)
	mustWrite(t, filepath.Join(old, "readme.txt"), []byte("cavs sdk fixture\n"))
	mustWrite(t, filepath.Join(newDir, "readme.txt"), []byte("cavs sdk fixture\n"))
	mustWrite(t, filepath.Join(newDir, "data/new_only.bin"), make([]byte, 64*1024))
	return old, newDir
}

func mustMkdir(t *testing.T, p string) {
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
}

func mustWrite(t *testing.T, p string, b []byte) {
	if err := os.WriteFile(p, b, 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestVersionAndCapabilities(t *testing.T) {
	if parts := strings.Split(Version(), "."); len(parts) != 3 {
		t.Errorf("version not semver: %q", Version())
	}
	if ABIVersion() != "1.0.0" {
		t.Errorf("abi = %q, want 1.0.0", ABIVersion())
	}
	var caps map[string]any
	if err := json.Unmarshal([]byte(CapabilitiesJSON()), &caps); err != nil {
		t.Fatalf("capabilities json: %v", err)
	}
	if caps["abiVersion"] != "1.0.0" {
		t.Errorf("caps abiVersion = %v", caps["abiVersion"])
	}
}

func TestEstimateSavings(t *testing.T) {
	c := newClient(t)
	rep, err := c.EstimateSavings(context.Background(), SavingsRequest{
		PricePerGB:               0.08,
		MonthlyDownloads:         500000,
		AverageFullDownloadBytes: 65011712,
		AverageCavsDownloadBytes: 2631921,
	})
	if err != nil {
		t.Fatal(err)
	}
	if rep.SavingsPercent < 90 {
		t.Errorf("savingsPercent = %v, want > 90", rep.SavingsPercent)
	}
	if rep.EstimatedMonthlySavings <= rep.CavsMonthlyCost {
		t.Errorf("savings %v should exceed cavs cost %v", rep.EstimatedMonthlySavings, rep.CavsMonthlyCost)
	}
}

func TestFullPipeline(t *testing.T) {
	c := newClient(t)
	ctx := context.Background()
	old, newDir := makeBuilds(t)
	work := t.TempDir()

	an, err := c.Analyze(ctx, AnalyzeRequest{OldPath: old, NewPath: newDir})
	if err != nil {
		t.Fatal(err)
	}
	if an.Summary.NewSizeBytes == 0 {
		t.Error("analyze: newSizeBytes is 0")
	}

	packOut := filepath.Join(work, "v2.cavs")
	pk, err := c.PackDirectory(ctx, PackDirectoryRequest{InputDir: newDir, OutputCavs: packOut})
	if err != nil {
		t.Fatal(err)
	}
	if pk.FilesPacked < 3 {
		t.Errorf("packed %d files, want >= 3", pk.FilesPacked)
	}

	planPath := filepath.Join(work, "update.cavsplan")
	pl, err := c.CreatePlan(ctx, CreatePlanRequest{OldPath: old, NewPath: newDir, OutputPlan: planPath})
	if err != nil {
		t.Fatal(err)
	}
	if pl.ReusedBytes == 0 {
		t.Error("plan found no reuse")
	}

	outDir := filepath.Join(work, "out")
	ap, err := c.ApplyPlan(ctx, ApplyPlanRequest{OldPath: old, PlanPath: planPath, OutputPath: outDir})
	if err != nil {
		t.Fatal(err)
	}
	if !ap.Verified {
		t.Error("apply not verified")
	}
	assertTreesEqual(t, newDir, outDir)

	pv, err := c.Preview(ctx, PreviewRequest{OldPath: old, NewPath: newDir})
	if err != nil {
		t.Fatal(err)
	}
	if len(pv.Routes) == 0 || pv.RecommendedRoute == "" {
		t.Error("preview returned no routes/recommendation")
	}

	bm, err := c.Benchmark(ctx, BenchmarkRequest{OldPath: old, NewPath: newDir, MeasureApply: false})
	if err != nil {
		t.Fatal(err)
	}
	if len(bm.Routes) != 4 {
		t.Errorf("benchmark routes = %d, want 4", len(bm.Routes))
	}
}

func TestVerifyInstall(t *testing.T) {
	c := newClient(t)
	ctx := context.Background()
	old, newDir := makeBuilds(t)
	work := t.TempDir()

	planPath := filepath.Join(work, "u.cavsplan")
	if _, err := c.CreatePlan(ctx, CreatePlanRequest{OldPath: old, NewPath: newDir, OutputPlan: planPath}); err != nil {
		t.Fatal(err)
	}
	outDir := filepath.Join(work, "out")
	if _, err := c.ApplyPlan(ctx, ApplyPlanRequest{OldPath: old, PlanPath: planPath, OutputPath: outDir}); err != nil {
		t.Fatal(err)
	}

	// A manifest-less verify needs a signature; make one via pack + manifest
	// digest is out of scope here, so verify against a directory signature
	// created by re-running createPlan analysis is not available. Instead
	// verify the negative path: a bogus signature path yields PATH-NOT-FOUND.
	_, err := c.VerifyInstall(ctx, VerifyRequest{Target: outDir, Signature: filepath.Join(work, "missing.cavssig")})
	if !IsCode(err, CodePathNotFound) {
		t.Fatalf("expected PATH-NOT-FOUND, got %v", err)
	}
}

func TestErrorMapping(t *testing.T) {
	c := newClient(t)
	_, err := c.Analyze(context.Background(), AnalyzeRequest{OldPath: "/no/such/old", NewPath: "/no/such/new"})
	if !IsCode(err, CodePathNotFound) {
		t.Fatalf("expected PATH-NOT-FOUND, got %v", err)
	}
	var ce *Error
	if e, ok := err.(*Error); ok {
		ce = e
	}
	if ce == nil || ce.Message == "" {
		t.Error("error missing message")
	}
}

func TestProgressCallback(t *testing.T) {
	c := newClient(t)
	old, newDir := makeBuilds(t)
	work := t.TempDir()
	var mu sync.Mutex
	var events []ProgressEvent
	_, err := c.CreatePlan(context.Background(),
		CreatePlanRequest{OldPath: old, NewPath: newDir, OutputPlan: filepath.Join(work, "p.cavsplan")},
		WithProgress(func(e ProgressEvent) {
			mu.Lock()
			events = append(events, e)
			mu.Unlock()
		}),
	)
	if err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(events) < 2 {
		t.Errorf("expected >= 2 progress events, got %d", len(events))
	}
	sawStarted := false
	for _, e := range events {
		if e.Type == "started" {
			sawStarted = true
		}
	}
	if !sawStarted {
		t.Error("never saw a 'started' event")
	}
}

func TestConcurrentClients(t *testing.T) {
	old, newDir := makeBuilds(t)
	var wg sync.WaitGroup
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			c, err := New()
			if err != nil {
				t.Error(err)
				return
			}
			defer c.Close()
			if _, err := c.Analyze(context.Background(), AnalyzeRequest{OldPath: old, NewPath: newDir}); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
}

func assertTreesEqual(t *testing.T, a, b string) {
	t.Helper()
	checked := 0
	err := filepath.Walk(a, func(path string, info os.FileInfo, err error) error {
		if err != nil || info.IsDir() {
			return err
		}
		rel, _ := filepath.Rel(a, path)
		want, _ := os.ReadFile(path)
		got, err := os.ReadFile(filepath.Join(b, rel))
		if err != nil {
			t.Errorf("missing %s in output", rel)
			return nil
		}
		if string(want) != string(got) {
			t.Errorf("content differs for %s", rel)
		}
		checked++
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if checked == 0 {
		t.Error("no files compared")
	}
}
