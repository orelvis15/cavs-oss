import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { CavsClient, CavsError, ErrorCode, version, abiVersion } from "../src/index";

function makeBuilds(): { oldDir: string; newDir: string; work: string } {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cavs-node-"));
  const oldDir = path.join(root, "Build_v1");
  const newDir = path.join(root, "Build_v2");
  fs.mkdirSync(path.join(oldDir, "data"), { recursive: true });
  fs.mkdirSync(path.join(newDir, "data"), { recursive: true });

  const base = Buffer.alloc(512 * 1024);
  for (let i = 0; i < base.length; i++) base[i] = i % 251;
  fs.writeFileSync(path.join(oldDir, "data/asset.bin"), base);
  const changed = Buffer.from(base);
  for (let i = 300000; i < 304096; i++) changed[i] ^= 0xff;
  fs.writeFileSync(path.join(newDir, "data/asset.bin"), changed);
  fs.writeFileSync(path.join(oldDir, "readme.txt"), "cavs sdk fixture\n");
  fs.writeFileSync(path.join(newDir, "readme.txt"), "cavs sdk fixture\n");
  fs.writeFileSync(path.join(newDir, "data/new_only.bin"), Buffer.alloc(64 * 1024));

  const work = path.join(root, "work");
  fs.mkdirSync(work);
  return { oldDir, newDir, work };
}

function assertTreesEqual(a: string, b: string): void {
  let checked = 0;
  const walk = (dir: string): void => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else {
        const rel = path.relative(a, full);
        assert.deepEqual(fs.readFileSync(full), fs.readFileSync(path.join(b, rel)), `differs: ${rel}`);
        checked++;
      }
    }
  };
  walk(a);
  assert.ok(checked > 0, "no files compared");
}

let client: CavsClient;
before(() => {
  client = new CavsClient();
});
after(() => {
  client?.close();
});

test("version and abi are semver", () => {
  assert.equal(version().split(".").length, 3);
  assert.equal(abiVersion(), "1.0.0");
});

test("estimateSavings", async () => {
  const r = await client.estimateSavings({
    pricePerGb: 0.08,
    monthlyDownloads: 500000,
    averageFullDownloadBytes: 65011712,
    averageCavsDownloadBytes: 2631921,
  });
  assert.ok(r.savingsPercent > 90, `savingsPercent=${r.savingsPercent}`);
  assert.ok(r.estimatedMonthlySavings > r.cavsMonthlyCost);
});

test("full pipeline: analyze → pack → plan → apply → preview → benchmark", async () => {
  const { oldDir, newDir, work } = makeBuilds();

  const an = await client.analyze({ oldPath: oldDir, newPath: newDir });
  assert.ok(an.summary.newSizeBytes > 0);

  const packOut = path.join(work, "v2.cavs");
  const pk = await client.packDirectory({ inputDir: newDir, outputCavs: packOut });
  assert.ok(pk.filesPacked >= 3);
  assert.ok(fs.existsSync(packOut));

  const planPath = path.join(work, "update.cavsplan");
  const pl = await client.createPlan({ oldPath: oldDir, newPath: newDir, outputPlan: planPath });
  assert.ok(pl.reusedBytes > 0, "plan found no reuse");

  const outDir = path.join(work, "out");
  const ap = await client.applyPlan({ oldPath: oldDir, planPath, outputPath: outDir });
  assert.ok(ap.verified);
  assertTreesEqual(newDir, outDir);

  const pv = await client.preview({ oldPath: oldDir, newPath: newDir, policy: "balanced" });
  assert.ok(pv.routes.length > 0);
  assert.ok(pv.recommendedRoute.length > 0);

  const bm = await client.benchmark({ oldPath: oldDir, newPath: newDir, measureApply: false });
  assert.equal(bm.routes.length, 4);
});

test("error mapping: missing path", async () => {
  await assert.rejects(
    client.analyze({ oldPath: "/no/such/old", newPath: "/no/such/new" }),
    (err: unknown) => {
      assert.ok(err instanceof CavsError);
      assert.equal(err.code, ErrorCode.PathNotFound);
      return true;
    },
  );
});

test("progress events are emitted", async () => {
  const { oldDir, newDir, work } = makeBuilds();
  const events: string[] = [];
  await client.createPlan(
    { oldPath: oldDir, newPath: newDir, outputPlan: path.join(work, "p.cavsplan") },
    { onProgress: (e) => events.push(e.type) },
  );
  assert.ok(events.length >= 2, `expected >= 2 events, got ${events.length}`);
  assert.ok(events.includes("started"));
});

test("AbortSignal cancels before start", async () => {
  const { oldDir, newDir, work } = makeBuilds();
  const ac = new AbortController();
  ac.abort();
  await assert.rejects(
    client.createPlan(
      { oldPath: oldDir, newPath: newDir, outputPlan: path.join(work, "c.cavsplan") },
      { signal: ac.signal },
    ),
    (err: unknown) => err instanceof CavsError && err.code === ErrorCode.Cancelled,
  );
});
