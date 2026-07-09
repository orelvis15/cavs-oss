import { useEffect, useState } from "react";
import { api, errMessage, pickPath } from "../../api/client";
import type { RequestLog, ServerStatus } from "../../api/types";
import { useI18n } from "../../i18n";
import { useStore } from "../../app/store";
import { useProjects } from "../../app/projects";
import { formatBytes } from "../../lib/format";
import { HelpPanel } from "../../components/HelpPanel";
import { CodeBlock } from "../../components/ui";
import { Icon } from "../../components/Icon";
import type { CustomPageProps } from "./types";

export function LocalServer({ sectionId }: CustomPageProps) {
  const { t, section } = useI18n();
  const { settings, notify } = useStore();
  const { currentProject } = useProjects();
  const [dir, setDir] = useState(currentProject?.outputFolder ?? settings.defaultOutputFolder ?? "");
  const [port, setPort] = useState(settings.localServerPort ?? 8990);
  const [status, setStatus] = useState<ServerStatus | null>(null);
  const [logs, setLogs] = useState<RequestLog[]>([]);
  const text = section(sectionId);

  const poll = async () => {
    try {
      const s = await api.serverStatus();
      setStatus(s);
      if (s.running) setLogs(await api.serverLogs());
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
      const s = await api.serverStart(dir, port);
      setStatus(s);
      notify("success", t("toast.serverStarted"));
    } catch (e) {
      notify("error", errMessage(e));
    }
  };
  const stop = async () => {
    const s = await api.serverStop();
    setStatus(s);
    setLogs([]);
    notify("info", t("toast.serverStopped"));
  };

  const running = status?.running ?? false;

  return (
    <div className="content-inner">
      <div className="page-head">
        <div>
          <h1 className="page-title">{text.label}</h1>
          <p className="page-tagline">{text.tagline}</p>
        </div>
      </div>
      <HelpPanel sectionId={sectionId} />

      <div className="rec warning" style={{ marginBottom: 16 }}>
        <p style={{ margin: 0 }}>{t("server.warning")}</p>
      </div>

      <div className="card" style={{ marginBottom: 16 }}>
        <div className="field">
          <label>{t("fields.serveDir")}</label>
          <div className="file-input">
            <input className="input mono" value={dir} placeholder={t("server.pickFolder")}
              onChange={(e) => setDir(e.target.value)} disabled={running} />
            <button className="btn" disabled={running}
              onClick={async () => { const p = await pickPath({ directory: true }); if (p) setDir(p); }}>
              <Icon name="folder-open" size={15} /> {t("common.browse")}
            </button>
          </div>
        </div>
        <div className="row" style={{ gap: 14, alignItems: "flex-end" }}>
          <div className="field" style={{ width: 140, marginBottom: 0 }}>
            <label>{t("fields.port")}</label>
            <input className="input" type="number" value={port} disabled={running}
              onChange={(e) => setPort(Number(e.target.value))} />
          </div>
          {!running ? (
            <button className="btn btn-primary" disabled={!dir} onClick={start}>
              <Icon name="play" size={15} /> {t("server.start")}
            </button>
          ) : (
            <button className="btn btn-danger" onClick={stop}>
              <Icon name="stop" size={15} /> {t("server.stop")}
            </button>
          )}
        </div>
      </div>

      <div className="card-grid grid-3" style={{ marginBottom: 16 }}>
        <div className="stat">
          <div className="stat-label">{t("server.status")}</div>
          <div className="stat-value" style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span className={"dot " + (running ? "green" : "gray")} />
            <span style={{ fontSize: 16 }}>{running ? t("server.running") : t("server.stopped")}</span>
          </div>
          {status?.url && <div className="stat-sub mono">{status.url}</div>}
        </div>
        <div className="stat">
          <div className="stat-label">{t("server.requests")}</div>
          <div className="stat-value">{status?.requests ?? 0}</div>
        </div>
        <div className="stat">
          <div className="stat-label">{t("server.bytes")}</div>
          <div className="stat-value">{formatBytes(status?.bytesServed ?? 0)}</div>
        </div>
      </div>

      {running && status?.url && (
        <div style={{ marginBottom: 16 }}>
          <div className="row" style={{ marginBottom: 8, gap: 8 }}>
            <button className="btn" onClick={() => navigator.clipboard.writeText(status.url!)}>
              <Icon name="copy" size={15} /> {t("server.copyUrl")}
            </button>
            <button className="btn" onClick={() => api.openPath(status.dir!)}>
              <Icon name="folder-open" size={15} /> {t("common.openFolder")}
            </button>
          </div>
          <CodeBlock lang="gdscript" code={godotConfig(status.url)} />
        </div>
      )}

      <div className="subhead">{t("server.logs")}</div>
      {logs.length === 0 ? (
        <div className="empty">—</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr><th>time</th><th>method</th><th>path</th><th>status</th><th>bytes</th><th>ms</th></tr>
            </thead>
            <tbody>
              {[...logs].reverse().slice(0, 60).map((l, i) => (
                <tr key={i}>
                  <td className="text-dim">{l.time}</td>
                  <td>{l.method}</td>
                  <td className="mono" style={{ maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{l.path}</td>
                  <td>
                    <span className={"badge " + (l.status < 400 ? "green" : "red")}>{l.status}</span>
                  </td>
                  <td>{formatBytes(l.bytes)}</td>
                  <td className="text-dim">{l.durationMs}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function godotConfig(url: string): string {
  return `Cavs.configure({
    "server_url": "${url}",
    "cache_dir": "user://cavs_cache",
    "packs_dir": "user://packs"
})`;
}
