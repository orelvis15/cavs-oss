// Command preview runs a CAVS update preview between two build directories.
//
//	go run ./examples/preview --old Build_v1 --new Build_v2
package main

import (
	"context"
	"flag"
	"fmt"
	"os"

	"github.com/orelvis15/cavs-oss/sdks/go/cavs"
)

func main() {
	old := flag.String("old", "", "old build path")
	newPath := flag.String("new", "", "new build path")
	flag.Parse()
	if *old == "" || *newPath == "" {
		fmt.Fprintln(os.Stderr, "usage: preview --old <dir> --new <dir>")
		os.Exit(2)
	}

	client, err := cavs.New()
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
	defer client.Close()

	report, err := client.Preview(context.Background(), cavs.PreviewRequest{
		OldPath: *old,
		NewPath: *newPath,
		Policy:  cavs.PolicyBalanced,
	}, cavs.WithProgress(func(e cavs.ProgressEvent) {
		if e.Phase != "" {
			fmt.Fprintf(os.Stderr, "  [%s] %s\n", e.Type, e.Phase)
		}
	}))
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}

	fmt.Printf("Recommended route: %s\n", report.RecommendedRoute)
	for _, r := range report.Routes {
		fmt.Printf("  %-16s %d bytes\n", r.Name, r.NetworkBytes)
	}
}
