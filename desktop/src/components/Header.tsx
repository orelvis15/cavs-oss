import { useI18n } from "../i18n";
import { useProjects } from "../app/projects";
import { ProjectAvatar } from "./ProjectAvatar";
import logo from "../assets/logo.png";

export function Header() {
  const { t } = useI18n();
  const { currentProject, selectProject } = useProjects();

  return (
    <header className="header">
      <div className="brand">
        <img className="logo-img" src={logo} alt="CAVS" />
        <div>
          <div className="title">{t("app.name")}</div>
          <div className="sub">{t("app.tagline")}</div>
        </div>
      </div>

      {currentProject && (
        <div className="header-actions">
          <div className="project-chip">
            <ProjectAvatar project={currentProject} size={24} />
            <div className="project-chip-text">
              <div className="project-chip-name">{currentProject.name}</div>
              <div className="project-chip-engine">{currentProject.engine}</div>
            </div>
          </div>
          <button className="btn" onClick={() => selectProject(null)}>
            {t("projects.switch")}
          </button>
        </div>
      )}
    </header>
  );
}
