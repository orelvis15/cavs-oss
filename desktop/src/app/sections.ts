// Section registry — the single source of truth for the sidebar, each
// section's "Create" behavior (wizard / compare / form / custom page) and
// which CAVS core operation it runs.

export type CreateKind = "wizard" | "compare" | "form" | "custom" | "none";

export interface FieldDef {
  key: string;
  /** i18n key under `fields`, or a literal fallback. */
  label: string;
  type: "file" | "folder" | "text" | "number" | "select";
  optional?: boolean;
  options?: { value: string; label: string }[];
  default?: string | number;
  placeholder?: string;
}

export interface CompareConfig {
  operation: string;
  oldKey: string;
  newKey: string;
  oldType: "file" | "folder";
  newType: "file" | "folder";
  engine?: boolean;
  engineDefault?: string;
  extraFields?: FieldDef[];
}

export interface WizardStep {
  id: string;
  title: string; // i18n key under sections.<id>.steps or literal
  fields: FieldDef[];
}

export interface WizardConfig {
  operation: string;
  steps: WizardStep[];
}

export interface FormConfig {
  operation: string;
  fields: FieldDef[];
}

export interface SectionDef {
  id: string;
  group: keyof typeof GROUPS;
  icon: string;
  create: CreateKind;
  compare?: CompareConfig;
  wizard?: WizardConfig;
  form?: FormConfig;
  /** For create === "custom": which custom page component to render. */
  custom?: string;
  /**
   * Explicit custom page that overrides the default SectionPage while still
   * keeping the section's `create` behavior (wizard/compare/form) available to
   * the page. Used by rich end-to-end flows such as Godot Runtime.
   */
  page?: string;
  /**
   * Engines this section is relevant to. When set, the section only appears
   * for projects using one of these engines. Absent = shown for all engines.
   */
  engines?: string[];
}

export const GROUPS = {
  start: "start",
  engine: "engine",
  analyze: "analyze",
  generate: "generate",
  plan: "plan",
  workspace: "workspace",
  integrate: "integrate",
  manage: "manage",
} as const;

const ENGINE_OPTIONS: { value: string; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "godot", label: "Godot" },
  { value: "generic", label: "Generic" },
  { value: "unity", label: "Unity" },
  { value: "unreal", label: "Unreal" },
];

const PROFILE_OPTIONS = [
  { value: "auto", label: "Auto (measured sweep)" },
  { value: "fastcdc-16k", label: "FastCDC 16k (smallest updates)" },
  { value: "fastcdc-32k", label: "FastCDC 32k" },
  { value: "fastcdc-64k", label: "FastCDC 64k" },
  { value: "fastcdc-64k-n3", label: "FastCDC 64k · norm-3 (tighter, new streams)" },
  { value: "fastcdc-128k-n3", label: "FastCDC 128k · norm-3 (tighter, new streams)" },
  { value: "fastcdc-256k", label: "FastCDC 256k" },
];

// Values must be the SDK's `zstd-<level>` / `none` strings.
const COMPRESSION_OPTIONS = [
  { value: "zstd-3", label: "zstd-3 (fast, default)" },
  { value: "zstd-9", label: "zstd-9" },
  { value: "zstd-19", label: "zstd-19 (smallest downloads)" },
  { value: "none", label: "none" },
];

export const SECTIONS: SectionDef[] = [
  // ---- Start ----
  { id: "home", group: "start", icon: "home", create: "custom", custom: "Dashboard" },
  { id: "activities", group: "start", icon: "list", create: "custom", custom: "Activities" },

  // ---- Engine (runtime flows) ----
  {
    id: "godot-runtime",
    group: "engine",
    icon: "rocket",
    create: "wizard",
    page: "GodotRuntime",
    engines: ["godot", "unity", "unreal"],
    wizard: {
      operation: "createPlan",
      steps: [
        {
          id: "pcks",
          title: "steps.pcks",
          fields: [
            { key: "oldPath", label: "fields.oldPath", type: "file" },
            { key: "newPath", label: "fields.newPath", type: "file" },
            { key: "assetName", label: "fields.assetName", type: "text", default: "game_content" },
            { key: "newVersion", label: "fields.version", type: "text", default: "1.0.1" },
          ],
        },
        {
          id: "output",
          title: "steps.output",
          fields: [
            { key: "outputPlan", label: "fields.outputPlan", type: "text", default: "update.cavsplan" },
          ],
        },
      ],
    },
  },
  {
    id: "godot-pck-analyzer",
    group: "engine",
    icon: "package",
    create: "compare",
    engines: ["godot"],
    compare: {
      operation: "analyze",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "file",
      newType: "file",
      engine: true,
      engineDefault: "godot",
    },
  },

  // ---- Analyze ----
  {
    id: "build-analyzer",
    group: "analyze",
    icon: "chart",
    create: "compare",
    compare: {
      operation: "analyze",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "folder",
      newType: "folder",
      engine: true,
      engineDefault: "auto",
    },
  },
  {
    id: "pack-inspector",
    group: "analyze",
    icon: "layers",
    create: "compare",
    compare: {
      operation: "analyze",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "file",
      newType: "file",
      engine: true,
      engineDefault: "generic",
    },
  },
  {
    id: "compare",
    group: "analyze",
    icon: "columns",
    create: "compare",
    compare: {
      operation: "analyze",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "folder",
      newType: "folder",
      engine: true,
      engineDefault: "auto",
    },
  },
  {
    id: "savings",
    group: "analyze",
    icon: "coins",
    create: "form",
    form: {
      operation: "estimateSavings",
      fields: [
        { key: "pricePerGb", label: "fields.pricePerGb", type: "number", default: 0.08 },
        { key: "monthlyDownloads", label: "fields.monthlyDownloads", type: "number", default: 500000 },
        { key: "averageFullDownloadBytes", label: "fields.averageFullDownloadBytes", type: "number", default: 134217728 },
        { key: "averageCavsDownloadBytes", label: "fields.averageCavsDownloadBytes", type: "number", default: 2621440 },
      ],
    },
  },

  // ---- Generate ----
  {
    id: "generate",
    group: "generate",
    icon: "sparkles",
    create: "form",
    form: {
      operation: "packDirectory",
      fields: [
        { key: "inputDir", label: "fields.inputDir", type: "folder" },
        { key: "outputCavs", label: "fields.outputCavs", type: "text", default: "release.cavs" },
        { key: "profile", label: "fields.profile", type: "select", options: PROFILE_OPTIONS, default: "fastcdc-64k" },
        { key: "compression", label: "fields.compression", type: "select", options: COMPRESSION_OPTIONS, default: "zstd-3" },
      ],
    },
  },
  {
    id: "apply-verify",
    group: "generate",
    icon: "check",
    create: "form",
    form: {
      operation: "applyPlan",
      fields: [
        { key: "oldPath", label: "fields.oldPath", type: "folder" },
        { key: "planPath", label: "fields.planPath", type: "file" },
        { key: "outputPath", label: "fields.outputPath", type: "text", default: "applied" },
      ],
    },
  },
  {
    id: "file-inspector",
    group: "generate",
    icon: "search",
    create: "form",
    form: {
      operation: "verifyInstall",
      fields: [{ key: "target", label: "fields.target", type: "file" }],
    },
  },

  // ---- Plan & compare ----
  {
    id: "publish-preview",
    group: "plan",
    icon: "eye",
    create: "compare",
    compare: {
      operation: "previewUpdate",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "folder",
      newType: "folder",
      engine: true,
      engineDefault: "auto",
    },
  },
  {
    id: "route-planner",
    group: "plan",
    icon: "route",
    create: "compare",
    compare: {
      operation: "previewUpdate",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "folder",
      newType: "folder",
      engine: true,
      engineDefault: "auto",
    },
  },
  {
    id: "benchmark",
    group: "plan",
    icon: "gauge",
    create: "compare",
    compare: {
      operation: "benchmark",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "file",
      newType: "file",
      engine: true,
      engineDefault: "auto",
    },
  },

  // ---- Workspace ----
  {
    id: "workspace",
    group: "workspace",
    icon: "grid",
    create: "form",
    form: {
      operation: "packDirectory",
      fields: [
        { key: "inputDir", label: "fields.inputDir", type: "folder" },
        { key: "outputCavs", label: "fields.outputCavs", type: "text", default: "depot.cavs" },
        { key: "profile", label: "fields.profile", type: "select", options: PROFILE_OPTIONS, default: "fastcdc-64k" },
      ],
    },
  },
  {
    id: "install-plan",
    group: "workspace",
    icon: "download",
    create: "compare",
    compare: {
      operation: "previewUpdate",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "folder",
      newType: "folder",
    },
  },
  {
    id: "shared-content",
    group: "workspace",
    icon: "share",
    create: "compare",
    compare: {
      operation: "analyze",
      oldKey: "oldPath",
      newKey: "newPath",
      oldType: "folder",
      newType: "folder",
      engine: true,
      engineDefault: "auto",
    },
  },
  { id: "build-history", group: "workspace", icon: "history", create: "custom", custom: "BuildHistory" },

  // ---- Integrate ----
  {
    id: "godot-plugin",
    group: "integrate",
    icon: "plug",
    create: "custom",
    custom: "PluginHelper",
    engines: ["godot", "unity", "unreal"],
  },
  { id: "local-server", group: "integrate", icon: "server", create: "custom", custom: "LocalServer" },
  { id: "serverless-cdn", group: "integrate", icon: "globe", create: "custom", custom: "ServerlessCdn" },
  { id: "sdk-helper", group: "integrate", icon: "code", create: "custom", custom: "SdkHelper" },
  { id: "cli-builder", group: "integrate", icon: "terminal", create: "custom", custom: "CliBuilder" },

  // ---- Manage ----
  { id: "reports", group: "manage", icon: "report", create: "custom", custom: "Reports" },
  { id: "recommendations", group: "manage", icon: "bulb", create: "custom", custom: "Recommendations" },
  { id: "engine-profiles", group: "manage", icon: "cube", create: "custom", custom: "CliInfo" },
  { id: "ignore-rules", group: "manage", icon: "filter", create: "custom", custom: "CliInfo" },
  {
    id: "security",
    group: "manage",
    icon: "shield",
    create: "form",
    form: {
      operation: "packDirectory",
      fields: [
        { key: "inputDir", label: "fields.inputDir", type: "folder" },
        { key: "outputCavs", label: "fields.outputCavs", type: "text", default: "signed-release.cavs" },
        { key: "signKeyPath", label: "fields.signKeyPath", type: "file", optional: true },
      ],
    },
  },
  { id: "cache", group: "manage", icon: "database", create: "custom", custom: "CliInfo" },
  { id: "export", group: "manage", icon: "export", create: "custom", custom: "CliInfo" },
  { id: "docs", group: "manage", icon: "book", create: "custom", custom: "Docs" },
  { id: "logs", group: "manage", icon: "list", create: "custom", custom: "Logs" },
  { id: "feedback", group: "manage", icon: "chat", create: "custom", custom: "Feedback" },
  { id: "settings", group: "manage", icon: "gear", create: "custom", custom: "Settings" },
];

export const SECTION_BY_ID: Record<string, SectionDef> = Object.fromEntries(
  SECTIONS.map((s) => [s.id, s])
);

export const ENGINE = ENGINE_OPTIONS;
