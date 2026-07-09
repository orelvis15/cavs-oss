import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useI18n } from "../../i18n";
import { useProjects } from "../../app/projects";
import { CodeBlock } from "../../components/ui";
import { Icon } from "../../components/Icon";
import { PageShell } from "./PageShell";
import type { CustomPageProps } from "./types";

const ISSUE_URL = "https://github.com/orelvis15/cavs/issues";
const DOCS_URL = "https://github.com/orelvis15/cavs-oss";

// ---------------- Projects ----------------
export function Projects({ sectionId, navigate }: CustomPageProps) {
  const { t } = useI18n();
  return (
    <PageShell sectionId={sectionId}>
      <div className="card-grid grid-3">
        {[
          { id: "godot-runtime", label: "Godot" },
          { id: "generate", label: "Generic" },
          { id: "build-analyzer", label: "Analyze" },
        ].map((p) => (
          <button key={p.id} className="tile" onClick={() => navigate(p.id)}>
            <h3>{p.label}</h3>
            <p>{t("common.create")}</p>
          </button>
        ))}
      </div>
    </PageShell>
  );
}

// ---------------- Plugin Helper (engine-aware) ----------------
export function PluginHelper({ sectionId }: CustomPageProps) {
  const { t, lang } = useI18n();
  const { currentProject } = useProjects();
  const engine = currentProject?.engine ?? "generic";

  // Only Godot ships a runtime plugin today.
  if (engine !== "godot") {
    const label = engine.charAt(0).toUpperCase() + engine.slice(1);
    return (
      <PageShell sectionId={sectionId}>
        <div className="card" style={{ textAlign: "center", padding: "40px 20px" }}>
          <div className="badge blue" style={{ marginBottom: 10 }}>
            {t("plugin.comingSoon")}
          </div>
          <p className="text-dim" style={{ maxWidth: 460, margin: "0 auto" }}>
            {lang === "es"
              ? `El plugin de runtime para ${label} aún no está disponible. Mientras tanto, usa los SDKs o el CLI para integrar CAVS en tu pipeline.`
              : `The ${label} runtime plugin is not available yet. In the meantime, use the SDKs or the CLI to integrate CAVS into your pipeline.`}
          </p>
        </div>
      </PageShell>
    );
  }

  return (
    <PageShell sectionId={sectionId}>
      <div className="subhead">Installation</div>
      <CodeBlock lang="text" code={`1. Copy the CAVS addon into res://addons/cavs/
2. Enable it in Project Settings → Plugins
3. Configure the client (below)`} />

      <div className="subhead">Minimal update_and_mount</div>
      <CodeBlock lang="gdscript" code={`Cavs.configure({
    "server_url": "http://localhost:8990",
    "cache_dir": "user://cavs_cache",
    "packs_dir": "user://packs"
})

var result = await Cavs.update_and_mount("game_content", "1.0.1")
if result.ok:
    print("Updated and mounted")
else:
    push_error(result.error)`} />

      <div className="subhead">Progress signals</div>
      <CodeBlock lang="gdscript" code={`Cavs.progress_changed.connect(_on_cavs_progress)
Cavs.update_completed.connect(_on_cavs_update_completed)
Cavs.update_failed.connect(_on_cavs_update_failed)

func _on_cavs_progress(phase: String, downloaded: int, total: int):
    $Label.text = phase
    if total > 0:
        $ProgressBar.value = float(downloaded) / float(total) * 100.0`} />
    </PageShell>
  );
}

// ---------------- SDK / Pipeline Helper ----------------
const SDK_EXAMPLES: Record<string, { install: string; example: string; lang: string }> = {
  Rust: {
    install: `cavs = "1.2"`,
    lang: "rust",
    example: `use cavs_sdk_core::dispatch;
let out = dispatch("analyze", &serde_json::json!({
    "oldPath": "build_old", "newPath": "build_new"
}), None, None)?;`,
  },
  Go: {
    install: `go get github.com/orelvis15/cavs/sdks/go`,
    lang: "go",
    example: `res, err := cavs.Analyze(cavs.AnalyzeRequest{
    OldPath: "build_old",
    NewPath: "build_new",
})`,
  },
  Java: {
    install: `implementation("org.cavs:cavs-sdk:1.2.0")`,
    lang: "kotlin",
    example: `val res = Cavs.analyze(AnalyzeRequest(
    oldPath = "build_old",
    newPath = "build_new",
))`,
  },
  "Node/TS": {
    install: `npm install @cavs/sdk`,
    lang: "ts",
    example: `import { analyze } from "@cavs/sdk";
const res = await analyze({ oldPath: "build_old", newPath: "build_new" });`,
  },
  CLI: {
    install: `cargo install cavs-cli`,
    lang: "bash",
    example: `cavs analyze build_old build_new
cavs pack build_new -o release.cavs`,
  },
};

const CI_TEMPLATES: Record<string, { lang: string; code: string }> = {
  "GitHub Actions": {
    lang: "yaml",
    code: `- name: Generate CAVS update
  run: |
    cavs pack ./build -o release_\${{ github.ref_name }}.cavs
    cavs verify release_\${{ github.ref_name }}.cavs`,
  },
  "Shell script": {
    lang: "bash",
    code: `#!/usr/bin/env bash
set -euo pipefail
cavs pack "$BUILD_DIR" -o "release_$VERSION.cavs"
cavs verify "release_$VERSION.cavs"`,
  },
};

export function SdkHelper({ sectionId }: CustomPageProps) {
  const [lang, setLang] = useState("Rust");
  const [ci, setCi] = useState("GitHub Actions");
  const sdk = SDK_EXAMPLES[lang];
  return (
    <PageShell sectionId={sectionId}>
      <div className="tabs">
        {Object.keys(SDK_EXAMPLES).map((k) => (
          <button key={k} className={"tab" + (k === lang ? " active" : "")} onClick={() => setLang(k)}>{k}</button>
        ))}
      </div>
      <div className="subhead" style={{ marginTop: 0 }}>Installation</div>
      <CodeBlock lang={sdk.lang} code={sdk.install} />
      <div className="subhead">Minimal example</div>
      <CodeBlock lang={sdk.lang} code={sdk.example} />

      <div className="subhead">Pipeline templates</div>
      <div className="tabs">
        {Object.keys(CI_TEMPLATES).map((k) => (
          <button key={k} className={"tab" + (k === ci ? " active" : "")} onClick={() => setCi(k)}>{k}</button>
        ))}
      </div>
      <CodeBlock lang={CI_TEMPLATES[ci].lang} code={CI_TEMPLATES[ci].code} />
    </PageShell>
  );
}

// ---------------- CLI Command Builder ----------------
export function CliBuilder({ sectionId }: CustomPageProps) {
  const { t } = useI18n();
  const [type, setType] = useState("pack");
  const [input, setInput] = useState("build_new");
  const [output, setOutput] = useState("release.cavs");
  const [asset, setAsset] = useState("game_content");
  const [version, setVersion] = useState("1.0.1");

  let cmd = "";
  if (type === "pack") cmd = `cavs pack ${input} --asset ${asset} --version ${version} -o ${output}`;
  else if (type === "analyze") cmd = `cavs analyze ${input}_old ${input}`;
  else if (type === "apply") cmd = `cavs apply ${input} plan.cavsplan -o applied`;
  else if (type === "serve") cmd = `cavs serve ./workspace --port 8990`;
  else if (type === "verify") cmd = `cavs verify ${output}`;

  return (
    <PageShell sectionId={sectionId}>
      <div className="card card-grid grid-2">
        <div className="field" style={{ marginBottom: 0 }}>
          <label>Command type</label>
          <select className="select" value={type} onChange={(e) => setType(e.target.value)}>
            {["pack", "analyze", "apply", "serve", "verify"].map((c) => <option key={c}>{c}</option>)}
          </select>
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("fields.inputDir")}</label>
          <input className="input" value={input} onChange={(e) => setInput(e.target.value)} />
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("fields.assetName")}</label>
          <input className="input" value={asset} onChange={(e) => setAsset(e.target.value)} />
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("fields.version")}</label>
          <input className="input" value={version} onChange={(e) => setVersion(e.target.value)} />
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>Output</label>
          <input className="input" value={output} onChange={(e) => setOutput(e.target.value)} />
        </div>
      </div>
      <div className="subhead">{t("result.cli")}</div>
      <CodeBlock lang="bash" code={cmd} />
    </PageShell>
  );
}

// ---------------- Docs ----------------
const DOC_SECTIONS = [
  { title: "Godot Quick Start", body: "Select two PCK files in Godot Runtime Update, generate a plan, start the local server and copy the GDScript snippet." },
  { title: "Runtime PCK Updates", body: "CAVS lets a Godot game download only what changed between two PCK versions, then mount the reconstructed pack at runtime." },
  { title: "CLI Basics", body: "cavs pack <dir> -o release.cavs · cavs analyze old new · cavs apply old plan -o out · cavs serve ./ws" },
  { title: "SDK Integration", body: "Every SDK wraps the same Rust core via a JSON-in/JSON-out surface. See the SDK / Pipeline Helper." },
  { title: "Troubleshooting", body: "Check Logs & Diagnostics for plain-language error explanations and suggested actions." },
];

export function Docs({ sectionId }: CustomPageProps) {
  return (
    <PageShell
      sectionId={sectionId}
      actions={
        <button className="btn" onClick={() => openUrl(DOCS_URL)}>
          <Icon name="external" size={15} /> Docs website
        </button>
      }
    >
      {DOC_SECTIONS.map((d) => (
        <div className="card" key={d.title} style={{ marginBottom: 10 }}>
          <h3 style={{ margin: "0 0 6px", fontSize: 14.5 }}>{d.title}</h3>
          <p className="text-dim" style={{ margin: 0 }}>{d.body}</p>
        </div>
      ))}
    </PageShell>
  );
}

// ---------------- Feedback ----------------
export function Feedback({ sectionId }: CustomPageProps) {
  const { t } = useI18n();
  const [tried, setTried] = useState("");
  const [happened, setHappened] = useState("");
  const [expected, setExpected] = useState("");

  const md = `## CAVS Desktop issue

### What I tried
${tried}

### What happened
${happened}

### Expected behavior
${expected}

### Environment
- CAVS Desktop
`;

  return (
    <PageShell sectionId={sectionId}>
      <div className="card">
        {[
          ["What were you trying to do?", tried, setTried],
          ["What happened?", happened, setHappened],
          ["What did you expect?", expected, setExpected],
        ].map(([label, val, set]: any) => (
          <div className="field" key={label}>
            <label>{label}</label>
            <textarea className="input" rows={3} value={val} onChange={(e) => set(e.target.value)} />
          </div>
        ))}
      </div>
      <div className="subhead">Generated issue</div>
      <CodeBlock lang="markdown" code={md} />
      <div style={{ marginTop: 12 }}>
        <button className="btn btn-primary" onClick={() => openUrl(ISSUE_URL)}>
          <Icon name="external" size={15} /> Open GitHub issue
        </button>
      </div>
      <p className="text-dim" style={{ fontSize: 12, marginTop: 8 }}>{t("common.copy")}: {ISSUE_URL}</p>
    </PageShell>
  );
}

// ---------------- CLI-managed sections ----------------
const CLI_HINTS: Record<string, { cmd: string }> = {
  workspace: { cmd: "cavs workspace init ./ws\ncavs workspace add-depot ./ws --name base" },
  "install-plan": { cmd: "cavs install-plan ./ws --platform windows --language en" },
  "shared-content": { cmd: "cavs analyze ./depotA ./depotB   # inspect shared %" },
  "engine-profiles": { cmd: "cavs analyze <old> <new> --engine godot" },
  "ignore-rules": { cmd: "# .cavsignore\n*.pdb\nlogs/\n*.tmp" },
  security: { cmd: "cavs signature keygen -o key\ncavs pack ./build --sign-key key -o release.cavs" },
  cache: { cmd: "cavs store info user://cavs_cache\ncavs store gc user://cavs_cache" },
  export: { cmd: "# Each operation stores result.json in its folder.\n# Use “Open folder” on any history entry." },
};

export function CliInfo({ sectionId }: CustomPageProps) {
  const { t } = useI18n();
  const hint = CLI_HINTS[sectionId];
  return (
    <PageShell sectionId={sectionId}>
      {hint && (
        <>
          <div className="subhead" style={{ marginTop: 0 }}>{t("result.cli")}</div>
          <CodeBlock lang="bash" code={hint.cmd} />
        </>
      )}
    </PageShell>
  );
}
