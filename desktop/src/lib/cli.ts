import { basename } from "./format";

// Best-effort "equivalent CLI command" for CLI transparency (spec §3.5).
export function cliEquivalent(kind: string, p: Record<string, any>): string | null {
  const q = (v: any) => {
    const s = String(v ?? "");
    return /\s/.test(s) ? `"${s}"` : s;
  };
  switch (kind) {
    case "analyze":
      return `cavs analyze ${q(p.oldPath)} ${q(p.newPath)}${p.engineHint && p.engineHint !== "auto" ? ` --engine ${p.engineHint}` : ""}`;
    case "packDirectory":
      return `cavs pack ${q(p.inputDir)} -o ${q(basename(p.outputCavs ?? "release.cavs"))}${p.profile ? ` --profile ${p.profile}` : ""}${p.compression ? ` --compression ${p.compression}` : ""}`;
    case "createPlan":
      return `cavs plan-update ${p.oldPath ? `--old ${q(p.oldPath)} ` : ""}--new ${q(p.newPath)} -o ${q(basename(p.outputPlan ?? "update.cavsplan"))}`;
    case "applyPlan":
      return `cavs apply ${q(p.oldPath)} ${q(p.planPath)} -o ${q(basename(p.outputPath ?? "applied"))}`;
    case "verifyInstall":
      return `cavs verify ${q(p.target)}${p.signature ? ` --signature ${q(p.signature)}` : ""}`;
    case "previewUpdate":
    case "compareRoutes":
      return `cavs publish-preview ${q(p.oldPath)} ${q(p.newPath)}`;
    case "benchmark":
      return `cavs bench-routes ${q(p.oldPath)} ${q(p.newPath)}`;
    case "estimateSavings":
      return null;
    default:
      return null;
  }
}
