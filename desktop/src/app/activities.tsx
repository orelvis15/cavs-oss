import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { api, errMessage } from "../api/client";
import { useI18n } from "../i18n";
import { useStore } from "./store";
import { useProjects } from "./projects";

export interface Activity {
  key: string;
  section: string;
  kind: string;
  title: string;
  status: "running" | "completed" | "failed";
  startedAt: string;
  finishedAt?: string;
  recordId?: string;
  error?: string;
}

interface ActivitiesValue {
  activities: Activity[];
  /** Bumps whenever an activity finishes, so history views can refetch. */
  tick: number;
  start: (req: { section: string; kind: string; title: string; params: Record<string, any> }) => void;
  remove: (key: string) => void;
}

const ActivitiesContext = createContext<ActivitiesValue | null>(null);

export function ActivitiesProvider({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  const { notify } = useStore();
  const { currentProject } = useProjects();
  const [activities, setActivities] = useState<Activity[]>([]);
  const [tick, setTick] = useState(0);

  const start = useCallback(
    (req: { section: string; kind: string; title: string; params: Record<string, any> }) => {
      if (!currentProject) {
        notify("error", t("toast.failed"));
        return;
      }
      const projectId = currentProject.id;
      const key =
        (globalThis.crypto?.randomUUID?.() as string) ?? String(Math.random()).slice(2);
      const startedAt = new Date().toISOString();
      setActivities((prev) => [
        { key, section: req.section, kind: req.kind, title: req.title, status: "running", startedAt },
        ...prev,
      ]);
      // Tell the user it runs in the background — no need to wait.
      notify("info", t("toast.background"));

      const finish = (patch: Partial<Activity>) => {
        setActivities((prev) =>
          prev.map((a) =>
            a.key === key ? { ...a, finishedAt: new Date().toISOString(), ...patch } : a
          )
        );
        setTick((x) => x + 1);
      };

      api
        .runOperation({ projectId, ...req })
        .then((rec) => {
          if (rec.status === "failed") {
            finish({ status: "failed", recordId: rec.id, error: rec.error?.message });
            notify("error", rec.error?.message ?? t("toast.failed"));
          } else {
            finish({ status: "completed", recordId: rec.id });
            notify("success", `${t("toast.done")}: ${rec.title}`);
          }
        })
        .catch((e) => {
          finish({ status: "failed", error: errMessage(e) });
          notify("error", errMessage(e));
        });
    },
    [notify, t, currentProject]
  );

  const remove = useCallback((key: string) => {
    setActivities((prev) => prev.filter((a) => a.key !== key));
  }, []);

  const value = useMemo<ActivitiesValue>(
    () => ({ activities, tick, start, remove }),
    [activities, tick, start, remove]
  );

  return <ActivitiesContext.Provider value={value}>{children}</ActivitiesContext.Provider>;
}

export function useActivities(): ActivitiesValue {
  const ctx = useContext(ActivitiesContext);
  if (!ctx) throw new Error("useActivities must be used within ActivitiesProvider");
  return ctx;
}
