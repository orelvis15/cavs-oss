import { useEffect, useMemo, useState } from "react";
import { api } from "../../api/client";
import type { OperationRecord } from "../../api/types";
import { useI18n } from "../../i18n";
import { useProjects } from "../../app/projects";
import { useActivities } from "../../app/activities";
import { formatDate } from "../../lib/format";
import { ResultView } from "../../components/ResultView";
import { Modal, StatusBadge, EmptyState } from "../../components/ui";
import { BarChart, type BarItem } from "../../components/charts";
import { useOperations } from "../../hooks/useOperations";
import { PageShell } from "./PageShell";
import type { CustomPageProps } from "./types";

// All operations for the current project, optionally filtered to sections.
function useAllOperations(sectionIds?: string[]) {
  const { currentProject } = useProjects();
  const { tick } = useActivities();
  const [records, setRecords] = useState<OperationRecord[]>([]);
  useEffect(() => {
    if (!currentProject) {
      setRecords([]);
      return;
    }
    api
      .listProjectOperations(currentProject.id)
      .then((rows) => {
        const filtered = sectionIds ? rows.filter((r) => sectionIds.includes(r.section)) : rows;
        setRecords(filtered);
      })
      .catch(() => setRecords([]));
  }, [currentProject?.id, tick, sectionIds?.join(",")]);
  return records;
}

const REPORT_SECTIONS = [
  "build-analyzer", "pack-inspector", "godot-pck-analyzer", "compare",
  "publish-preview", "route-planner", "benchmark", "savings", "apply-verify", "file-inspector",
];

// ---------------- Reports ----------------
export function Reports({ sectionId }: CustomPageProps) {
  const { t, section, lang } = useI18n();
  const records = useAllOperations(REPORT_SECTIONS);
  const [q, setQ] = useState("");
  const [view, setView] = useState<OperationRecord | null>(null);

  const filtered = useMemo(
    () => records.filter((r) => (r.title + r.section + r.kind).toLowerCase().includes(q.toLowerCase())),
    [records, q]
  );

  return (
    <PageShell sectionId={sectionId}>
      <input className="input" placeholder={t("common.search")} value={q}
        onChange={(e) => setQ(e.target.value)} style={{ marginBottom: 14 }} />
      {filtered.length === 0 ? (
        <EmptyState text={t("history.empty")} />
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr><th>Section</th><th>{t("history.columns.title")}</th><th>{t("history.columns.date")}</th><th>{t("history.columns.status")}</th></tr>
            </thead>
            <tbody>
              {filtered.map((r) => (
                <tr key={r.id} className="clickable" onClick={() => setView(r)}>
                  <td style={{ fontWeight: 600 }}>{section(r.section).label}</td>
                  <td>{r.title}</td>
                  <td className="text-dim">{formatDate(r.createdAt, lang)}</td>
                  <td><StatusBadge status={r.status} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {view && (
        <Modal title={view.title} onClose={() => setView(null)} wide>
          <ResultView record={view} />
        </Modal>
      )}
    </PageShell>
  );
}

// ---------------- Recommendations ----------------
export function Recommendations({ sectionId }: CustomPageProps) {
  const { t, section } = useI18n();
  const records = useAllOperations(["build-analyzer", "pack-inspector", "godot-pck-analyzer", "compare"]);

  const items = records.flatMap((r) => {
    const recs = Array.isArray(r.result?.recommendations) ? r.result.recommendations : [];
    return recs.map((rec: any) => ({ rec, source: r }));
  });

  return (
    <PageShell sectionId={sectionId}>
      {items.length === 0 ? (
        <EmptyState text={t("history.empty")} />
      ) : (
        items.map(({ rec, source }, i) => (
          <div key={i} className={"rec " + sevClass(rec.severity)}>
            <div className="row spread">
              <h4>{rec.title}</h4>
              <span className="badge gray">{section(source.section).label}</span>
            </div>
            {rec.why && <p>{rec.why}</p>}
            {rec.file && <p className="mono">{rec.file}</p>}
            {rec.fix && <p className="rec-fix">→ {rec.fix}</p>}
          </div>
        ))
      )}
    </PageShell>
  );
}

// ---------------- Build History ----------------
export function BuildHistory({ sectionId }: CustomPageProps) {
  const { t, lang } = useI18n();
  const { records } = useOperations("generate");

  const bars: BarItem[] = records
    .slice(0, 10)
    .reverse()
    .map((r) => {
      const bytes = firstByteValue(r.result) ?? 0;
      return { label: r.title, value: bytes };
    })
    .filter((b) => b.value > 0);

  return (
    <PageShell sectionId={sectionId}>
      {bars.length > 0 && (
        <div className="card" style={{ marginBottom: 16 }}>
          <div className="subhead" style={{ marginTop: 0 }}>Update size over time</div>
          <BarChart items={bars} />
        </div>
      )}
      {records.length === 0 ? (
        <EmptyState text={t("history.empty")} />
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr><th>{t("history.columns.title")}</th><th>{t("history.columns.date")}</th><th>{t("history.columns.files")}</th><th>{t("history.columns.status")}</th></tr>
            </thead>
            <tbody>
              {records.map((r) => (
                <tr key={r.id}>
                  <td style={{ fontWeight: 600 }}>{r.title}</td>
                  <td className="text-dim">{formatDate(r.createdAt, lang)}</td>
                  <td className="text-dim">{r.files.join(", ") || "—"}</td>
                  <td><StatusBadge status={r.status} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </PageShell>
  );
}

// ---------------- Logs ----------------
export function Logs({ sectionId }: CustomPageProps) {
  const { t, section, lang } = useI18n();
  const records = useAllOperations();
  const failures = records.filter((r) => r.status === "failed");

  return (
    <PageShell sectionId={sectionId}>
      {failures.length === 0 ? (
        <EmptyState text={t("history.empty")} />
      ) : (
        failures.map((r) => (
          <div key={r.id} className="rec critical">
            <div className="row spread">
              <h4>{r.error?.code ?? "ERROR"}</h4>
              <span className="text-dim" style={{ fontSize: 12 }}>
                {section(r.section).label} · {formatDate(r.createdAt, lang)}
              </span>
            </div>
            <p>{r.error?.message}</p>
          </div>
        ))
      )}
    </PageShell>
  );
}

function sevClass(sev: string): string {
  if (sev === "critical" || sev === "error") return "critical";
  if (sev === "warning") return "warning";
  return "info";
}

function firstByteValue(obj: any): number | null {
  if (!obj || typeof obj !== "object") return null;
  for (const [k, v] of Object.entries(obj)) {
    if (typeof v === "number" && /bytes|size/i.test(k)) return v;
    if (v && typeof v === "object") {
      const nested = firstByteValue(v);
      if (nested) return nested;
    }
  }
  return null;
}
