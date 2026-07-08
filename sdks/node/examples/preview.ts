// Run a CAVS update preview between two build directories.
//
//   npm run build && node dist/examples/preview.js --old Build_v1 --new Build_v2
import { CavsClient } from "../src/index";

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const oldPath = valueOf(args, "--old");
  const newPath = valueOf(args, "--new");
  if (!oldPath || !newPath) {
    console.error("usage: preview --old <dir> --new <dir>");
    process.exit(2);
  }

  const cavs = new CavsClient();
  try {
    const report = await cavs.preview(
      { oldPath, newPath, policy: "balanced" },
      { onProgress: (e) => e.phase && console.error(`  [${e.type}] ${e.phase}`) },
    );
    console.log(`Recommended route: ${report.recommendedRoute}`);
    for (const r of report.routes) {
      console.log(`  ${r.name.padEnd(16)} ${r.networkBytes} bytes`);
    }
  } finally {
    cavs.close();
  }
}

function valueOf(args: string[], flag: string): string | undefined {
  const i = args.indexOf(flag);
  return i >= 0 ? args[i + 1] : undefined;
}

void main();
