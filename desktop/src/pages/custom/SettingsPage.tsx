import { useEffect, useState } from "react";
import { api } from "../../api/client";
import type { Lang, Theme, ToolStatus } from "../../api/types";
import { useI18n } from "../../i18n";
import { useStore } from "../../app/store";
import { HelpPanel } from "../../components/HelpPanel";
import { Icon } from "../../components/Icon";
import type { CustomPageProps } from "./types";

export function SettingsPage({ sectionId }: CustomPageProps) {
  const { t, section } = useI18n();
  const { settings, updateSettings, appInfo } = useStore();
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loadingTools, setLoadingTools] = useState(false);
  const text = section(sectionId);

  const detect = async () => {
    setLoadingTools(true);
    try {
      setTools(await api.detectTools());
    } finally {
      setLoadingTools(false);
    }
  };
  useEffect(() => { detect(); }, []);

  return (
    <div className="content-inner">
      <div className="page-head">
        <div>
          <h1 className="page-title">{text.label}</h1>
          <p className="page-tagline">{text.tagline}</p>
        </div>
      </div>
      <HelpPanel sectionId={sectionId} />

      <div className="subhead">{t("settings.general")}</div>
      <div className="card card-grid grid-2">
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("config.language")}</label>
          <select className="select" value={settings.language}
            onChange={(e) => updateSettings({ language: e.target.value as Lang })}>
            <option value="es">{t("config.spanish")}</option>
            <option value="en">{t("config.english")}</option>
          </select>
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("config.theme")}</label>
          <select className="select" value={settings.theme}
            onChange={(e) => updateSettings({ theme: e.target.value as Theme })}>
            <option value="dark">{t("config.themeDark")}</option>
            <option value="light">{t("config.themeLight")}</option>
          </select>
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("fields.port")}</label>
          <input className="input" type="number" value={settings.localServerPort}
            onChange={(e) => updateSettings({ localServerPort: Number(e.target.value) })} />
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>{t("result.cli")}</label>
          <label className="row" style={{ fontWeight: 400, gap: 8 }}>
            <input type="checkbox" checked={settings.showCliPreview}
              onChange={(e) => updateSettings({ showCliPreview: e.target.checked })} />
            <span className="text-dim">{t("result.cli")}</span>
          </label>
        </div>
      </div>

      <div className="subhead row spread">
        <span>{t("settings.tools")}</span>
        <button className="btn btn-ghost" onClick={detect} disabled={loadingTools}>
          {loadingTools ? <span className="loader" /> : <Icon name="refresh" size={15} />}
        </button>
      </div>
      <div className="table-wrap">
        <table className="tbl">
          <thead>
            <tr><th>Tool</th><th>{t("server.status")}</th><th>Version</th><th>Path</th></tr>
          </thead>
          <tbody>
            {loadingTools && tools.length === 0 && (
              <tr>
                <td colSpan={4} className="text-dim" style={{ textAlign: "center", padding: "18px" }}>
                  <span className="loader" style={{ marginRight: 8, verticalAlign: "middle" }} />
                  {t("common.loading")}
                </td>
              </tr>
            )}
            {tools.map((tool) => (
              <tr key={tool.name}>
                <td style={{ fontWeight: 600 }}>{tool.name}</td>
                <td>
                  <span className={"badge " + (tool.available ? "green" : "gray")}>
                    {tool.available ? t("settings.detected") : t("settings.missing")}
                  </span>
                </td>
                <td className="text-dim mono" style={{ fontSize: 11.5 }}>{tool.version ?? "—"}</td>
                <td className="text-dim mono" style={{ fontSize: 11 }}>{tool.path ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="subhead">{t("settings.core")}</div>
      <div className="card">
        <dl className="kv">
          <dt>App version</dt><dd>{appInfo?.appVersion ?? "—"}</dd>
          <dt>CAVS core</dt><dd>{appInfo?.sdkVersion ?? "—"}</dd>
          <dt>ABI</dt><dd>{appInfo?.abiVersion ?? "—"}</dd>
          <dt>Platform</dt><dd>{appInfo ? `${appInfo.os} / ${appInfo.arch}` : "—"}</dd>
        </dl>
      </div>

      <div className="subhead">{t("settings.privacy")}</div>
      <div className="rec info">
        <p style={{ margin: 0 }}>{t("settings.privacyText")}</p>
      </div>
    </div>
  );
}
