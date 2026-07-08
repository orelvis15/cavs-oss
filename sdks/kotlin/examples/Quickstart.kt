// Runs the full CAVS update lifecycle against two synthetic builds it
// generates for you, so you can see the SDK end to end with zero setup:
//
//   analyze → preview → createPlan → applyPlan → estimateSavings
//
// Run it from the Kotlin SDK root, pointing the loader at a local native
// build (see the examples README):
//
//   cargo build --release -p cavs-ffi          # from the repo root
//   export CAVS_SDK_LIBRARY="$PWD/target/release/libcavs_sdk.dylib"   # or .so
//   gradle -q runQuickstart
//
// Everything is written under a temporary directory that is removed on exit.
package com.cavs.examples

import com.cavs.sdk.CavsClient
import com.cavs.sdk.model.AnalyzeRequest
import com.cavs.sdk.model.ApplyPlanRequest
import com.cavs.sdk.model.CreatePlanRequest
import com.cavs.sdk.model.PreviewRequest
import com.cavs.sdk.model.SavingsRequest
import java.nio.file.Files

fun main() {
    val work = Files.createTempDirectory("cavs-quickstart-")
    try {
        println("Generating synthetic builds under $work")
        val (v1, v2) = generateBuilds(work)

        CavsClient.create().use { cavs ->
            // 1) Analyze — how much of v2 can be reused from v1?
            println("\n== analyze ==")
            val report = cavs.analyze(AnalyzeRequest(oldPath = v1.toString(), newPath = v2.toString()))
            val s = report.summary
            println("  unchanged=${s.filesUnchanged} modified=${s.filesModified} added=${s.filesAdded} deleted=${s.filesDeleted}")
            println("  full new build: ${human(s.newSizeBytes)}")
            println("  CAVS update:    ${human(s.estimatedUpdateBytes)} (reuse ${"%.1f".format(s.cavsReuseRatio * 100)}%)")

            // 2) Preview — what does each delivery route cost on the wire?
            println("\n== preview ==")
            val preview = cavs.preview(
                PreviewRequest(oldPath = v1.toString(), newPath = v2.toString(), policy = "balanced"),
            )
            println("  recommended route: ${preview.recommendedRoute}")
            for (r in preview.routes) {
                println("    ${r.name.padEnd(16)} ${human(r.networkBytes)}")
            }

            // 3) Create a portable update plan (a .cavsplan file).
            println("\n== createPlan ==")
            val planPath = work.resolve("v1_to_v2.cavsplan").toString()
            val plan = cavs.createPlan(
                CreatePlanRequest(oldPath = v1.toString(), newPath = v2.toString(), outputPlan = planPath),
            )
            println("  wrote v1_to_v2.cavsplan (${human(plan.estimatedNetworkBytes)} on the wire)")
            println("  reused ${human(plan.reusedBytes)} from old build, ${human(plan.inlineBytes)} of fresh data")

            // 4) Apply the plan to v1 to reconstruct v2 (the client-side update).
            println("\n== applyPlan ==")
            val outDir = work.resolve("Applied_v2").toString()
            val apply = cavs.applyPlan(
                ApplyPlanRequest(oldPath = v1.toString(), planPath = planPath, outputPath = outDir, deleteRemoved = true),
            )
            println("  reconstructed ${apply.filesTotal} files (verified=${apply.verified})")
            println("  ${human(apply.bytesFromOld)} reused from disk, only ${human(apply.bytesFromBlob)} came from the download")

            // 5) Estimate what that reuse is worth in egress at scale.
            println("\n== estimateSavings ==")
            val savings = cavs.estimateSavings(
                SavingsRequest(
                    pricePerGb = 0.09, // typical CDN egress $/GB
                    monthlyDownloads = 1_000_000.0,
                    averageFullDownloadBytes = s.newSizeBytes.toDouble(),
                    averageCavsDownloadBytes = s.estimatedUpdateBytes.toDouble(),
                ),
            )
            println("  full re-download: $${"%.2f".format(savings.fullDownloadMonthlyCost)}/mo")
            println("  with CAVS:        $${"%.2f".format(savings.cavsMonthlyCost)}/mo")
            println("  saved:            $${"%.2f".format(savings.estimatedMonthlySavings)}/mo (${"%.1f".format(savings.savingsPercent)}%)")

            println("\nDone.")
        }
    } finally {
        work.toFile().deleteRecursively()
    }
}
