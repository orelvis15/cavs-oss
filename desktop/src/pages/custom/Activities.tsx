import { useEffect, useState } from "react";
import { api } from "../../api/client";
import type { OperationRecord } from "../../api/types";
import { useI18n } from "../../i18n";
import { useActivities } from "../../app/activities";
import { useProjects } from "../../app/projects";
import { formatDate } from "../../lib/format";
import { ResultView } from "../../components/ResultView";
import { Modal, StatusBadge, EmptyState } from "../../components/ui";
import { PageShell } from "./PageShell";
import type { CustomPageProps } from "./types";

export function Activities({ sectionId, navigate }: CustomPageProps) {
  const { t, section, lang } = useI18n();
  const { activities, tick, remove } = useActivities();
  const { currentProject } = useProjects();
  const [done, setDone] = useState<OperationRecord[]>([]);
  const [view, setView] = useState<OperationRecord | null>(null);

  useEffect(() => {
    if (!currentProject) return;
    api
      .listProjectOperations(currentProject.id)
      .then(setDone)
      .catch(() => setDone([]));
  }, [currentProject?.id, tick]);

  const running = activities.filter((a) => a.status === "running");

  return (
    <PageShell sectionId={sectionId}>
      <div className="subhead" style={{ marginTop: 0 }}>{t("activities.inProgress")}</div>
      {running.length === 0 ? (
        <div className="text-dim" style={{ marginBottom: 18 }}>{t("activities.noneRunning")}</div>
      ) : (
        <div className="table-wrap" style={{ marginBottom: 18 }}>
          <table className="tbl">
            <tbody>
              {running.map((a) => (
                <tr key={a.key}>
                  <td style={{ width: 26 }}><span className="loader" /></td>
                  <td style={{ fontWeight: 600 }}>{a.title}</td>
                  <td>{section(a.section).label}</td>
                  <td className="text-dim">{formatDate(a.startedAt, lang)}</td>
                  <td style={{ textAlign: "right" }}>
                    <button className="btn btn-ghost" onClick={() => navigate(a.section)}>
                      {t("activities.goToSection")}
                    </button>
                    <button className="btn btn-ghost" onClick={() => remove(a.key)}>
                      {t("common.close")}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="subhead">{t("activities.completed")}</div>
      {done.length === 0 ? (
        <EmptyState text={t("history.empty")} />
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>{t("history.columns.title")}</th>
                <th>{t("activities.section")}</th>
                <th>{t("history.columns.date")}</th>
                <th>{t("history.columns.status")}</th>
                <th style={{ textAlign: "right" }}>{t("history.columns.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {done.map((r) => (
                <tr key={r.id}>
                  <td style={{ fontWeight: 600 }}>{r.title}</td>
                  <td>{section(r.section).label}</td>
                  <td className="text-dim">{formatDate(r.createdAt, lang)}</td>
                  <td><StatusBadge status={r.status} /></td>
                  <td>
                    <div className="row-actions">
                      <button className="btn btn-ghost" onClick={() => setView(r)}>
                        {t("common.info")}
                      </button>
                      <button className="btn btn-ghost" onClick={() => navigate(r.section)}>
                        {t("activities.goToSection")}
                      </button>
                    </div>
                  </td>
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
