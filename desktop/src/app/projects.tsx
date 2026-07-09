import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { api, errMessage } from "../api/client";
import type { NewProject, Project } from "../api/types";
import { useStore } from "./store";

interface ProjectsValue {
  ready: boolean;
  projects: Project[];
  currentProject: Project | null;
  selectProject: (id: string | null) => void;
  refresh: () => Promise<void>;
  create: (p: NewProject) => Promise<Project | null>;
  update: (p: Project) => Promise<Project | null>;
  remove: (id: string) => Promise<void>;
}

const ProjectsContext = createContext<ProjectsValue | null>(null);

export function ProjectsProvider({ children }: { children: ReactNode }) {
  const { notify } = useStore();
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [currentId, setCurrentId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setProjects(await api.listProjects());
    } catch {
      setProjects([]);
    }
  }, []);

  useEffect(() => {
    (async () => {
      await refresh();
      setReady(true);
    })();
  }, [refresh]);

  const create = useCallback(
    async (p: NewProject) => {
      try {
        const created = await api.createProject(p);
        await refresh();
        return created;
      } catch (e) {
        notify("error", errMessage(e));
        return null;
      }
    },
    [refresh, notify]
  );

  const update = useCallback(
    async (p: Project) => {
      try {
        const updated = await api.updateProject(p);
        await refresh();
        return updated;
      } catch (e) {
        notify("error", errMessage(e));
        return null;
      }
    },
    [refresh, notify]
  );

  const remove = useCallback(
    async (id: string) => {
      try {
        await api.deleteProject(id);
        if (currentId === id) setCurrentId(null);
        await refresh();
      } catch (e) {
        notify("error", errMessage(e));
      }
    },
    [refresh, notify, currentId]
  );

  const currentProject = useMemo(
    () => projects.find((p) => p.id === currentId) ?? null,
    [projects, currentId]
  );

  const value = useMemo<ProjectsValue>(
    () => ({
      ready,
      projects,
      currentProject,
      selectProject: setCurrentId,
      refresh,
      create,
      update,
      remove,
    }),
    [ready, projects, currentProject, refresh, create, update, remove]
  );

  return <ProjectsContext.Provider value={value}>{children}</ProjectsContext.Provider>;
}

export function useProjects(): ProjectsValue {
  const ctx = useContext(ProjectsContext);
  if (!ctx) throw new Error("useProjects must be used within ProjectsProvider");
  return ctx;
}
