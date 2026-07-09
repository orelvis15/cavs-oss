import { useEffect, useState } from "react";
import { useStore } from "./store";
import { useProjects } from "./projects";
import { SECTION_BY_ID } from "./sections";
import { Header } from "../components/Header";
import { Sidebar } from "../components/Sidebar";
import { Toasts } from "../components/ui";
import { SectionPage } from "../pages/SectionPage";
import { CUSTOM_PAGES } from "../pages/custom";
import { ProjectsLanding } from "../pages/ProjectsLanding";

export function App() {
  const { ready } = useStore();
  const { ready: projectsReady, currentProject } = useProjects();
  const [active, setActive] = useState("home");

  // Always land on the dashboard when entering a project.
  useEffect(() => {
    if (currentProject) setActive("home");
  }, [currentProject?.id]);

  if (!ready || !projectsReady) {
    return (
      <div style={{ height: "100vh", display: "grid", placeItems: "center" }}>
        <span className="loader" style={{ width: 26, height: 26 }} />
      </div>
    );
  }

  // No project selected → the Projects landing is the first screen.
  if (!currentProject) {
    return (
      <>
        <ProjectsLanding />
        <Toasts />
      </>
    );
  }

  const section = SECTION_BY_ID[active] ?? SECTION_BY_ID.home;

  return (
    <div className="app-shell">
      <Header />
      <div className="app-body">
        <Sidebar active={active} onSelect={setActive} />
        <main className="content">
          {/* Remount content when switching project so per-project state resets. */}
          <Content key={currentProject.id} sectionId={section.id} navigate={setActive} />
        </main>
      </div>
      <Toasts />
    </div>
  );
}

function Content({
  sectionId,
  navigate,
}: {
  sectionId: string;
  navigate: (id: string) => void;
}) {
  const section = SECTION_BY_ID[sectionId];
  const pageName = section.page ?? (section.create === "custom" ? section.custom : undefined);
  if (pageName) {
    const Page = CUSTOM_PAGES[pageName];
    if (Page) return <Page sectionId={sectionId} navigate={navigate} />;
  }
  return <SectionPage section={section} />;
}
