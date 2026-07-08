// Runs the full CAVS update lifecycle against two synthetic builds it
// generates for you, so you can see the SDK end to end with zero setup:
//
//   analyze → preview → createPlan → applyPlan → estimateSavings
//
// Build the SDK and run it:
//
//   npm run native   # local dev: build + stage the native library
//   npm run build
//   node dist/examples/endToEnd.js
//
// Everything is written under a temporary directory that is removed on exit.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CavsClient } from "../src/index";
import { generate, human } from "./sample";

async function main(): Promise<void> {
  const work = mkdtempSync(join(tmpdir(), "cavs-quickstart-"));
  const cavs = new CavsClient();
  try {
    console.log("Generating synthetic builds under", work);
    const { v1, v2 } = generate(work);

    // 1) Analyze — how much of v2 can be reused from v1?
    console.log("\n== analyze ==");
    const report = await cavs.analyze({ oldPath: v1, newPath: v2 });
    const s = report.summary;
    console.log(`  unchanged=${s.filesUnchanged} modified=${s.filesModified} added=${s.filesAdded} deleted=${s.filesDeleted}`);
    console.log(`  full new build: ${human(s.newSizeBytes)}`);
    console.log(`  CAVS update:    ${human(s.estimatedUpdateBytes)} (reuse ${(s.cavsReuseRatio * 100).toFixed(1)}%)`);

    // 2) Preview — what does each delivery route cost on the wire?
    console.log("\n== preview ==");
    const preview = await cavs.preview({ oldPath: v1, newPath: v2, policy: "balanced" });
    console.log(`  recommended route: ${preview.recommendedRoute}`);
    for (const r of preview.routes) {
      console.log(`    ${r.name.padEnd(16)} ${human(r.networkBytes)}`);
    }

    // 3) Create a portable update plan (a .cavsplan file).
    console.log("\n== createPlan ==");
    const planPath = join(work, "v1_to_v2.cavsplan");
    const plan = await cavs.createPlan({ oldPath: v1, newPath: v2, outputPlan: planPath });
    console.log(`  wrote v1_to_v2.cavsplan (${human(plan.estimatedNetworkBytes)} on the wire)`);
    console.log(`  reused ${human(plan.reusedBytes)} from old build, ${human(plan.inlineBytes)} of fresh data`);

    // 4) Apply the plan to v1 to reconstruct v2 (the client-side update).
    console.log("\n== applyPlan ==");
    const outDir = join(work, "Applied_v2");
    const apply = await cavs.applyPlan({ oldPath: v1, planPath, outputPath: outDir, deleteRemoved: true });
    console.log(`  reconstructed ${apply.filesTotal} files (verified=${apply.verified})`);
    console.log(`  ${human(apply.bytesFromOld)} reused from disk, only ${human(apply.bytesFromBlob)} came from the download`);

    // 5) Estimate what that reuse is worth in egress at scale.
    console.log("\n== estimateSavings ==");
    const savings = await cavs.estimateSavings({
      pricePerGb: 0.09, // typical CDN egress $/GB
      monthlyDownloads: 1_000_000,
      averageFullDownloadBytes: s.newSizeBytes,
      averageCavsDownloadBytes: s.estimatedUpdateBytes,
    });
    console.log(`  full re-download: $${savings.fullDownloadMonthlyCost.toFixed(2)}/mo`);
    console.log(`  with CAVS:        $${savings.cavsMonthlyCost.toFixed(2)}/mo`);
    console.log(`  saved:            $${savings.estimatedMonthlySavings.toFixed(2)}/mo (${savings.savingsPercent.toFixed(1)}%)`);

    console.log("\nDone.");
  } finally {
    cavs.close();
    rmSync(work, { recursive: true, force: true });
  }
}

void main();
