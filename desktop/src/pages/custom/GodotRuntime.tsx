import { useEffect, useState } from "react";
import { api, errMessage } from "../../api/client";
import type { OperationRecord, ServerStatus } from "../../api/types";
import { SECTION_BY_ID } from "../../app/sections";
import { useI18n } from "../../i18n";
import { useStore } from "../../app/store";
import { useProjects } from "../../app/projects";
import { useOperations } from "../../hooks/useOperations";
import { formatBytes } from "../../lib/format";
import { HelpPanel } from "../../components/HelpPanel";
import { HistoryTable } from "../../components/HistoryTable";
import { CreateModal } from "../../components/CreateModal";
import { Modal, CodeBlock } from "../../components/ui";
import { Donut, BarChart, type BarItem } from "../../components/charts";
import { Icon } from "../../components/Icon";
import type { CustomPageProps } from "./types";

export function GodotRuntime({ sectionId, navigate }: CustomPageProps) {
  const { t, lang, section } = useI18n();
  const { currentProject } = useProjects();
  const { records, refresh } = useOperations(sectionId);
  const [creating, setCreating] = useState(false);
  const [viewing, setViewing] = useState<OperationRecord | null>(null);
  const def = SECTION_BY_ID[sectionId];
  const text = section(sectionId);
  const engine = currentProject?.engine ?? "godot";

  // Only Godot ships a runtime flow today; other engines show "coming soon".
  if (engine !== "godot") {
    const label = engine.charAt(0).toUpperCase() + engine.slice(1);
    return (
      <div className="content-inner">
        <div className="page-head">
          <div>
            <h1 className="page-title">{text.label}</h1>
            <p className="page-tagline">{text.tagline}</p>
          </div>
        </div>
        <HelpPanel sectionId={sectionId} />
        <div className="card" style={{ textAlign: "center", padding: "40px 20px" }}>
          <div className="badge blue" style={{ marginBottom: 10 }}>{t("plugin.comingSoon")}</div>
          <p className="text-dim" style={{ maxWidth: 460, margin: "0 auto" }}>
            {lang === "es"
              ? `El flujo de actualización en runtime para ${label} aún no está disponible. Mientras tanto puedes usar Analizar, Generar y el Servidor local.`
              : `The ${label} runtime update flow is not available yet. In the meantime you can use Analyze, Generate and the Local server.`}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="content-inner">
      <div className="page-head">
        <div>
          <h1 className="page-title">{text.label}</h1>
          <p className="page-tagline">{text.tagline}</p>
        </div>
        <button className="btn btn-primary btn-lg" onClick={() => setCreating(true)}>
          <Icon name="plus" size={17} />
          {t("common.create")}
        </button>
      </div>

      <HelpPanel sectionId={sectionId} />

      <FlowStrip />

      <div className="subhead" style={{ marginTop: 8 }}>{t("history.title")}</div>
      <HistoryTable records={records} onOpen={setViewing} onChanged={refresh} />

      {creating && <CreateModal section={def} onClose={() => setCreating(false)} />}

      {viewing && (
        <Modal title={`${text.label} — ${viewing.title}`} onClose={() => setViewing(null)} wide>
          <GodotRuntimeDetail record={viewing} navigate={navigate} />
        </Modal>
      )}
    </div>
  );
}

function FlowStrip() {
  const steps = ["PCKs", "Analyze", "Generate", "Serve", "Snippet", "Test"];
  return (
    <div className="card" style={{ marginBottom: 18 }}>
      <div className="stepper" style={{ margin: 0 }}>
        {steps.map((s, i) => (
          <div key={s} style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <div className="step-chip">
              <span className="num">{i + 1}</span>
              {s}
            </div>
            {i < steps.length - 1 && <span className="step-sep" />}
          </div>
        ))}
      </div>
    </div>
  );
}

function GodotRuntimeDetail({
  record,
  navigate,
}: {
  record: OperationRecord;
  navigate: (id: string) => void;
}) {
  const { t, lang } = useI18n();
  const { settings } = useStore();
  const r = record.result ?? {};

  const fullSize = r.expectedOutputSize ?? 0;
  const updateSize = r.estimatedNetworkBytes ?? r.planBytes ?? 0;
  const savings = fullSize > 0 ? (1 - updateSize / fullSize) * 100 : 0;
  const friendliness = patchFriendliness(savings);

  const failed = record.status === "failed";

  return (
    <div>
      {failed && record.error && (
        <div className="rec critical" style={{ marginBottom: 16 }}>
          <h4>{record.error.code}</h4>
          <p>{record.error.message}</p>
        </div>
      )}

      {!failed && (
        <>
          {/* Step 2 — Analyze */}
          <div className="subhead" style={{ marginTop: 0 }}>
            {lang === "es" ? "Análisis" : "Analysis"}
          </div>
          <div className="card-grid grid-3" style={{ marginBottom: 8 }}>
            <Stat label={lang === "es" ? "PCK completo" : "Full PCK"} value={formatBytes(fullSize)} />
            <Stat label={lang === "es" ? "Actualización CAVS" : "CAVS update"} value={formatBytes(updateSize)} />
            <Stat
              label={lang === "es" ? "Aptitud para parches" : "Patch-friendliness"}
              value={friendliness.label[lang as "es" | "en"]}
              sub={`${r.operationCount ?? 0} ops · ${r.copyOps ?? 0} copy`}
            />
          </div>
          <div className="card" style={{ marginBottom: 8 }}>
            <div className="row spread wrap" style={{ gap: 20 }}>
              <Donut percent={savings} label={lang === "es" ? "Ahorro estimado" : "Estimated savings"} />
              <div style={{ flex: 1, minWidth: 260 }}>
                <BarChart
                  items={
                    [
                      { label: lang === "es" ? "Descarga completa" : "Full download", value: fullSize, color: "gray" },
                      { label: lang === "es" ? "Reutilizado" : "Reused", value: r.reusedBytes ?? 0, color: "green" },
                      { label: lang === "es" ? "Actualización CAVS" : "CAVS update", value: updateSize, color: "accent" },
                    ] as BarItem[]
                  }
                />
              </div>
            </div>
          </div>

          {/* Step 3 — Generated files */}
          <FilesCard record={record} />

          {/* Step 4 & 5 — Serve + Snippet */}
          <ServeAndSnippet record={record} port={settings.localServerPort} />

          {/* Step 6 — Test checklist */}
          <TestChecklist recordId={record.id} onOpenServer={() => navigate("local-server")} />
        </>
      )}

      <details style={{ marginTop: 16 }}>
        <summary style={{ cursor: "pointer", color: "var(--text-dim)", fontSize: 12.5 }}>
          {t("result.raw")}
        </summary>
        <pre className="code" style={{ marginTop: 8 }}>
          {JSON.stringify({ params: record.params, result: record.result }, null, 2)}
        </pre>
      </details>
    </div>
  );
}

function FilesCard({ record }: { record: OperationRecord }) {
  const { t } = useI18n();
  const { notify } = useStore();
  const open = async () => {
    try {
      await api.openPath(record.artifactDir);
    } catch (e) {
      notify("error", errMessage(e));
    }
  };
  return (
    <>
      <div className="subhead">{t("result.outputs")}</div>
      <div className="card row spread" style={{ marginBottom: 8 }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          {record.files.length ? (
            record.files.map((f) => <span key={f} className="mono">{f}</span>)
          ) : (
            <span className="text-dim">{t("result.noFiles")}</span>
          )}
        </div>
        <button className="btn" onClick={open}>
          <Icon name="folder-open" size={15} />
          {t("common.openFolder")}
        </button>
      </div>
    </>
  );
}

function ServeAndSnippet({ record, port }: { record: OperationRecord; port: number }) {
  const { t, lang } = useI18n();
  const { notify } = useStore();
  const [status, setStatus] = useState<ServerStatus | null>(null);

  const servingThis =
    status?.running && status.dir && sameDir(status.dir, record.artifactDir);

  const poll = async () => {
    try {
      setStatus(await api.serverStatus());
    } catch {
      /* ignore */
    }
  };
  useEffect(() => {
    poll();
    const id = setInterval(poll, 1500);
    return () => clearInterval(id);
  }, []);

  const start = async () => {
    try {
      const s = await api.serverStart(record.artifactDir, port);
      setStatus(s);
      notify("success", t("toast.serverStarted"));
    } catch (e) {
      notify("error", errMessage(e));
    }
  };
  const stop = async () => {
    setStatus(await api.serverStop());
    notify("info", t("toast.serverStopped"));
  };

  const url = servingThis && status?.url ? status.url : `http://localhost:${port}`;
  const asset = record.params?.assetName ?? "game_content";
  const version = record.params?.newVersion ?? "1.0.1";

  return (
    <>
      <div className="subhead">{lang === "es" ? "Servidor de prueba local" : "Local test server"}</div>
      <div className="card row spread wrap" style={{ marginBottom: 8, gap: 10 }}>
        <div className="row" style={{ gap: 10 }}>
          <span className={"dot " + (servingThis ? "green" : "gray")} />
          <span className="mono">{servingThis ? url : t("server.stopped")}</span>
        </div>
        <div className="row" style={{ gap: 8 }}>
          {!servingThis ? (
            <button className="btn btn-primary" onClick={start}>
              <Icon name="play" size={15} /> {t("server.start")}
            </button>
          ) : (
            <>
              <button className="btn" onClick={() => navigator.clipboard.writeText(url)}>
                <Icon name="copy" size={15} /> {t("server.copyUrl")}
              </button>
              <button className="btn btn-danger" onClick={stop}>
                <Icon name="stop" size={15} /> {t("server.stop")}
              </button>
            </>
          )}
        </div>
      </div>
      <p className="text-dim" style={{ fontSize: 12, margin: "0 0 10px" }}>
        {t("server.warning")}
      </p>

      <div className="subhead">{lang === "es" ? "Snippet de Godot" : "Godot snippet"}</div>
      <CodeBlock
        lang="gdscript"
        code={`Cavs.configure({
    "server_url": "${url}",
    "cache_dir": "user://cavs_cache",
    "packs_dir": "user://packs"
})

var result = await Cavs.update_and_mount("${asset}", "${version}")
if result.ok:
    print("Updated and mounted")
else:
    push_error(result.error)`}
      />
    </>
  );
}

const CHECKLIST_KEYS = [
  { en: "Local server running", es: "Servidor local en ejecución" },
  { en: "Godot project opened", es: "Proyecto de Godot abierto" },
  { en: "Plugin installed", es: "Plugin instalado" },
  { en: "Snippet added", es: "Snippet agregado" },
  { en: "Update button clicked in game", es: "Botón de actualizar pulsado en el juego" },
  { en: "New PCK mounted successfully", es: "Nuevo PCK montado correctamente" },
];

function TestChecklist({
  recordId,
  onOpenServer,
}: {
  recordId: string;
  onOpenServer: () => void;
}) {
  const { lang } = useI18n();
  const storageKey = `cavs-godot-checklist-${recordId}`;
  const [checked, setChecked] = useState<boolean[]>(() => {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw) return JSON.parse(raw);
    } catch {
      /* ignore */
    }
    return CHECKLIST_KEYS.map(() => false);
  });

  const toggle = (i: number) => {
    setChecked((prev) => {
      const next = prev.map((v, j) => (j === i ? !v : v));
      localStorage.setItem(storageKey, JSON.stringify(next));
      return next;
    });
  };

  return (
    <>
      <div className="subhead row spread">
        <span>{lang === "es" ? "Checklist de prueba" : "Test checklist"}</span>
        <button className="btn btn-ghost" onClick={onOpenServer} style={{ fontWeight: 500 }}>
          <Icon name="server" size={14} /> {lang === "es" ? "Ir al servidor" : "Go to server"}
        </button>
      </div>
      <div className="card">
        {CHECKLIST_KEYS.map((item, i) => (
          <label
            key={i}
            className="row"
            style={{ gap: 10, padding: "6px 0", cursor: "pointer", fontWeight: 400 }}
          >
            <input type="checkbox" checked={checked[i]} onChange={() => toggle(i)} />
            <span style={{ textDecoration: checked[i] ? "line-through" : "none", color: checked[i] ? "var(--text-faint)" : "var(--text)" }}>
              {item[lang as "es" | "en"]}
            </span>
          </label>
        ))}
      </div>
    </>
  );
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="stat">
      <div className="stat-label">{label}</div>
      <div className="stat-value" style={{ fontSize: 18 }}>{value}</div>
      {sub && <div className="stat-sub">{sub}</div>}
    </div>
  );
}

function patchFriendliness(savings: number): { label: { es: string; en: string } } {
  if (savings >= 90) return { label: { es: "Excelente", en: "Excellent" } };
  if (savings >= 70) return { label: { es: "Buena", en: "Good" } };
  if (savings >= 40) return { label: { es: "Regular", en: "Fair" } };
  return { label: { es: "Baja", en: "Poor" } };
}

function sameDir(a: string, b: string): boolean {
  const norm = (s: string) => s.replace(/[\\/]+$/, "");
  return norm(a) === norm(b);
}
