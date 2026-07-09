import { useEffect, useState } from "react";
import { api, errMessage } from "../../api/client";
import type { OperationRecord } from "../../api/types";
import { useI18n } from "../../i18n";
import { useStore } from "../../app/store";
import { useProjects } from "../../app/projects";
import { useActivities } from "../../app/activities";
import { ProjectAvatar } from "../../components/ProjectAvatar";
import { formatDate, truncateMiddle } from "../../lib/format";
import type { CustomPageProps } from "./types";

const QUICK_GODOT = ["godot-runtime", "build-analyzer", "generate", "apply-verify", "local-server", "publish-preview"];
const QUICK_OTHER = ["build-analyzer", "generate", "apply-verify", "compare", "local-server", "publish-preview"];

export function Dashboard({ navigate }: CustomPageProps) {
  const { t, section, lang } = useI18n();
  const { notify } = useStore();
  const { currentProject } = useProjects();
  const { tick } = useActivities();
  const [ops, setOps] = useState<OperationRecord[]>([]);

  useEffect(() => {
    if (!currentProject) return;
    api.listProjectOperations(currentProject.id).then(setOps).catch(() => setOps([]));
  }, [currentProject?.id, tick]);

  if (!currentProject) return null;

  const quick = currentProject.engine === "godot" ? QUICK_GODOT : QUICK_OTHER;
  const releases = ops.filter((o) => o.section === "generate").length;
  const analyses = ops.filter((o) => o.section === "build-analyzer" || o.section === "pack-inspector").length;
  const last = ops[0];

  const openFolder = async () => {
    try {
      await api.openPath(currentProject.outputFolder);
    } catch (e) {
      notify("error", errMessage(e));
    }
  };

  return (
    <div className="content-inner">
      <div className="card project-hero" style={{ marginBottom: 20 }}>
        <ProjectAvatar project={currentProject} size={54} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <h1 className="page-title" style={{ margin: 0 }}>{currentProject.name}</h1>
          <div className="row wrap" style={{ gap: 8, marginTop: 6 }}>
            <span className="badge blue">{currentProject.engine}</span>
            <span className="mono text-dim" title={currentProject.outputFolder}>
              {truncateMiddle(currentProject.outputFolder, 52)}
            </span>
          </div>
        </div>
        <button className="btn" onClick={openFolder}>{t("common.openFolder")}</button>
      </div>

      <div className="card-grid grid-3" style={{ marginBottom: 22 }}>
        <div className="stat">
          <div className="stat-label">{t("dashboard.releases")}</div>
          <div className="stat-value">{releases}</div>
        </div>
        <div className="stat">
          <div className="stat-label">{t("dashboard.analyses")}</div>
          <div className="stat-value">{analyses}</div>
        </div>
        <div className="stat">
          <div className="stat-label">{t("dashboard.lastActivity")}</div>
          <div className="stat-value" style={{ fontSize: 15 }}>
            {last ? formatDate(last.createdAt, lang) : "—"}
          </div>
        </div>
      </div>

      <div className="subhead" style={{ marginTop: 0 }}>{t("dashboard.quickActions")}</div>
      <div className="card-grid grid-auto">
        {quick.map((id) => {
          const s = section(id);
          return (
            <button key={id} className="tile" onClick={() => navigate(id)}>
              <h3>{s.label}</h3>
              <p>{s.tagline}</p>
            </button>
          );
        })}
      </div>
    </div>
  );
}
