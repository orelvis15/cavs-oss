import { useState } from "react";
import { pickPath } from "../api/client";
import type { Lang, Project } from "../api/types";
import { useI18n } from "../i18n";
import { useStore } from "../app/store";
import { useProjects } from "../app/projects";
import { Modal, EmptyState } from "../components/ui";
import { ProjectAvatar } from "../components/ProjectAvatar";
import { truncateMiddle } from "../lib/format";
import logo from "../assets/logo.png";

const ENGINES = ["godot", "generic", "unity", "unreal", "custom"];
const ICONS = ["🎮", "🕹️", "🚀", "🛰️", "🧩", "⚙️", "📦", "🗺️", "🌍", "🔥", "⭐", "🎯"];

export function ProjectsLanding() {
  const { t } = useI18n();
  const { projects, selectProject, remove } = useProjects();
  const [editing, setEditing] = useState<Project | "new" | null>(null);

  return (
    <div className="landing">
      <LandingTopBar />

      <div className="landing-body">
        <div className="page-head">
          <div>
            <h1 className="page-title">{t("projects.title")}</h1>
            <p className="page-tagline">{t("projects.tagline")}</p>
          </div>
          <button className="btn btn-primary btn-lg" onClick={() => setEditing("new")}>
            {t("projects.new")}
          </button>
        </div>

        {projects.length === 0 ? (
          <EmptyState text={t("projects.empty")} />
        ) : (
          <div className="card-grid grid-auto">
            {projects.map((p) => (
              <div key={p.id} className="project-card" onClick={() => selectProject(p.id)}>
                <div className="project-card-top">
                  <ProjectAvatar project={p} size={44} />
                  <span className="badge gray">{p.engine}</span>
                </div>
                <h3 className="project-card-name">{p.name}</h3>
                <div className="project-card-path mono" title={p.outputFolder}>
                  {truncateMiddle(p.outputFolder, 40)}
                </div>
                <div className="project-card-actions" onClick={(e) => e.stopPropagation()}>
                  <button className="btn btn-ghost" onClick={() => setEditing(p)}>
                    {t("common.edit")}
                  </button>
                  <button
                    className="btn btn-danger"
                    onClick={() => {
                      if (window.confirm(t("projects.confirmDelete"))) remove(p.id);
                    }}
                  >
                    {t("common.delete")}
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {editing && (
        <ProjectModal
          project={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}

function LandingTopBar() {
  const { t, lang } = useI18n();
  const { settings, updateSettings } = useStore();
  return (
    <div className="landing-topbar">
      <div className="brand">
        <img className="logo-img" src={logo} alt="CAVS" />
        <div>
          <div className="title">{t("app.name")}</div>
          <div className="sub">{t("app.tagline")}</div>
        </div>
      </div>
      <div className="header-actions">
        <div className="seg">
          {(["es", "en"] as Lang[]).map((l) => (
            <button
              key={l}
              className={"seg-btn" + (lang === l ? " active" : "")}
              onClick={() => updateSettings({ language: l })}
            >
              {l.toUpperCase()}
            </button>
          ))}
        </div>
        <div className="seg">
          <button
            className={"seg-btn" + (settings.theme === "dark" ? " active" : "")}
            onClick={() => updateSettings({ theme: "dark" })}
          >
            {t("config.themeDark")}
          </button>
          <button
            className={"seg-btn" + (settings.theme === "light" ? " active" : "")}
            onClick={() => updateSettings({ theme: "light" })}
          >
            {t("config.themeLight")}
          </button>
        </div>
      </div>
    </div>
  );
}

function ProjectModal({ project, onClose }: { project: Project | null; onClose: () => void }) {
  const { t } = useI18n();
  const { create, update, selectProject } = useProjects();
  const [name, setName] = useState(project?.name ?? "");
  const [folder, setFolder] = useState(project?.outputFolder ?? "");
  const [engine, setEngine] = useState(project?.engine ?? "godot");
  const [icon, setIcon] = useState<string | null>(project?.icon ?? null);
  const [busy, setBusy] = useState(false);

  const valid = name.trim() !== "" && folder.trim() !== "";

  const submit = async () => {
    if (!valid) return;
    setBusy(true);
    if (project) {
      const updated = await update({ ...project, name, outputFolder: folder, engine, icon });
      setBusy(false);
      if (updated) onClose();
    } else {
      const created = await create({ name, outputFolder: folder, engine, icon });
      setBusy(false);
      if (created) {
        onClose();
        selectProject(created.id);
      }
    }
  };

  return (
    <Modal
      title={project ? t("projects.editTitle") : t("projects.newTitle")}
      onClose={onClose}
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button className="btn btn-primary" disabled={!valid || busy} onClick={submit}>
            {busy && <span className="loader" />} {project ? t("common.save") : t("common.create")}
          </button>
        </>
      }
    >
      <div className="field">
        <label>{t("projects.name")} <span className="hint">({t("common.required")})</span></label>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} autoFocus />
      </div>

      <div className="field">
        <label>{t("projects.folder")} <span className="hint">({t("common.required")})</span></label>
        <div className="file-input">
          <input className="input mono" value={folder} placeholder={t("common.selectFolder")}
            onChange={(e) => setFolder(e.target.value)} />
          <button className="btn" onClick={async () => {
            const p = await pickPath({ directory: true, title: t("projects.folder") });
            if (p) setFolder(p);
          }}>
            {t("common.browse")}
          </button>
        </div>
        <div className="hint" style={{ marginTop: 6 }}>{t("projects.folderHint")}</div>
      </div>

      <div className="field">
        <label>{t("projects.engine")}</label>
        <select className="select" value={engine} onChange={(e) => setEngine(e.target.value)}>
          {ENGINES.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
      </div>

      <div className="field">
        <label>{t("projects.icon")} <span className="hint">({t("common.optional")})</span></label>
        <div className="icon-picker">
          <button
            className={"icon-opt" + (icon === null ? " active" : "")}
            onClick={() => setIcon(null)}
            title={t("common.none")}
          >
            {name.trim().charAt(0).toUpperCase() || "?"}
          </button>
          {ICONS.map((em) => (
            <button
              key={em}
              className={"icon-opt" + (icon === em ? " active" : "")}
              onClick={() => setIcon(em)}
            >
              {em}
            </button>
          ))}
        </div>
      </div>
    </Modal>
  );
}
