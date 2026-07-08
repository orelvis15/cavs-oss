// Command quickstart runs the full CAVS update lifecycle against two
// synthetic builds it generates for you, so you can see the SDK end to end
// with zero setup:
//
//	analyze → preview → createPlan → applyPlan → estimateSavings
//
// Run it from the Go SDK root (after `make native`):
//
//	make native
//	go run ./examples/quickstart
//
// Everything is written under a temporary directory that is removed on exit.
package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/orelvis15/cavs-oss/sdks/go/cavs"
	"github.com/orelvis15/cavs-oss/sdks/go/examples/internal/sample"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func run() error {
	work, err := os.MkdirTemp("", "cavs-quickstart-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(work)

	fmt.Println("Generating synthetic builds under", work)
	v1, v2, err := sample.Generate(work)
	if err != nil {
		return err
	}

	client, err := cavs.New()
	if err != nil {
		return err
	}
	defer client.Close()

	ctx := context.Background()

	// 1) Analyze — how much of v2 can be reused from v1?
	fmt.Println("\n== analyze ==")
	report, err := client.Analyze(ctx, cavs.AnalyzeRequest{OldPath: v1.Dir, NewPath: v2.Dir})
	if err != nil {
		return err
	}
	s := report.Summary
	fmt.Printf("  unchanged=%d modified=%d added=%d deleted=%d\n",
		s.FilesUnchanged, s.FilesModified, s.FilesAdded, s.FilesDeleted)
	fmt.Printf("  full new build: %s\n", human(s.NewSizeBytes))
	fmt.Printf("  CAVS update:    %s (reuse %.1f%%)\n",
		human(s.EstimatedUpdateBytes), s.CavsReuseRatio*100)

	// 2) Preview — what does each delivery route cost on the wire?
	fmt.Println("\n== preview ==")
	preview, err := client.Preview(ctx, cavs.PreviewRequest{
		OldPath: v1.Dir, NewPath: v2.Dir, Policy: cavs.PolicyBalanced,
	})
	if err != nil {
		return err
	}
	fmt.Printf("  recommended route: %s\n", preview.RecommendedRoute)
	for _, r := range preview.Routes {
		fmt.Printf("    %-16s %s\n", r.Name, human(r.NetworkBytes))
	}

	// 3) Create a portable update plan (a .cavsplan file).
	fmt.Println("\n== createPlan ==")
	planPath := filepath.Join(work, "v1_to_v2.cavsplan")
	plan, err := client.CreatePlan(ctx, cavs.CreatePlanRequest{
		OldPath: v1.Dir, NewPath: v2.Dir, OutputPlan: planPath,
	})
	if err != nil {
		return err
	}
	fmt.Printf("  wrote %s (%s on the wire)\n",
		filepath.Base(plan.PlanPath), human(plan.EstimatedNetworkBytes))
	fmt.Printf("  reused %s from old build, %s of fresh data\n",
		human(plan.ReusedBytes), human(plan.InlineBytes))

	// 4) Apply the plan to v1 to reconstruct v2 (the client-side update).
	fmt.Println("\n== applyPlan ==")
	outDir := filepath.Join(work, "Applied_v2")
	apply, err := client.ApplyPlan(ctx, cavs.ApplyPlanRequest{
		OldPath: v1.Dir, PlanPath: planPath, OutputPath: outDir, DeleteRemoved: true,
	})
	if err != nil {
		return err
	}
	fmt.Printf("  reconstructed %d files (verified=%t)\n", apply.FilesTotal, apply.Verified)
	fmt.Printf("  %s reused from disk, only %s came from the download\n",
		human(apply.BytesFromOld), human(apply.BytesFromBlob))

	// 5) Estimate what that reuse is worth in egress at scale.
	fmt.Println("\n== estimateSavings ==")
	savings, err := client.EstimateSavings(ctx, cavs.SavingsRequest{
		PricePerGB:               0.09, // typical CDN egress $/GB
		MonthlyDownloads:         1_000_000,
		AverageFullDownloadBytes: float64(s.NewSizeBytes),
		AverageCavsDownloadBytes: float64(s.EstimatedUpdateBytes),
	})
	if err != nil {
		return err
	}
	fmt.Printf("  full re-download: $%.2f/mo\n", savings.FullDownloadMonthlyCost)
	fmt.Printf("  with CAVS:        $%.2f/mo\n", savings.CavsMonthlyCost)
	fmt.Printf("  saved:            $%.2f/mo (%.1f%%)\n",
		savings.EstimatedMonthlySavings, savings.SavingsPercent)

	fmt.Println("\nDone.")
	return nil
}

func human(b uint64) string {
	const unit = 1024
	if b < unit {
		return fmt.Sprintf("%d B", b)
	}
	div, exp := uint64(unit), 0
	for n := b / unit; n >= unit; n /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.1f %ciB", float64(b)/float64(div), "KMGTPE"[exp])
}
