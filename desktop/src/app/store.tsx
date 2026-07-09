import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { api } from "../api/client";
import type { AppInfo, Settings } from "../api/types";
import { I18nProvider } from "../i18n";

export interface Toast {
  id: number;
  kind: "success" | "error" | "info";
  message: string;
}

interface StoreValue {
  ready: boolean;
  settings: Settings;
  appInfo: AppInfo | null;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
  toasts: Toast[];
  notify: (kind: Toast["kind"], message: string) => void;
  dismiss: (id: number) => void;
}

const DEFAULT_SETTINGS: Settings = {
  language: "es",
  theme: "dark",
  defaultOutputFolder: null,
  localServerPort: 8990,
  showCliPreview: true,
  recentProjectsLimit: 10,
};

const StoreContext = createContext<StoreValue | null>(null);

let toastSeq = 1;

export function AppProvider({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState(false);
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);

  useEffect(() => {
    (async () => {
      try {
        const [s, info] = await Promise.all([api.getSettings(), api.appInfo()]);
        setSettings({ ...DEFAULT_SETTINGS, ...s });
        setAppInfo(info);
      } catch {
        // Fall back to defaults if the backend is not reachable yet.
      } finally {
        setReady(true);
      }
    })();
  }, []);

  // Apply theme to the document root.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", settings.theme);
    document.documentElement.setAttribute("lang", settings.language);
  }, [settings.theme, settings.language]);

  const notify = useCallback((kind: Toast["kind"], message: string) => {
    const id = toastSeq++;
    setToasts((prev) => [...prev, { id, kind, message }]);
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4200);
  }, []);

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const updateSettings = useCallback(
    async (patch: Partial<Settings>) => {
      const next = { ...settings, ...patch };
      setSettings(next);
      try {
        await api.saveSettings(next);
      } catch {
        notify("error", "Could not save settings");
      }
    },
    [settings, notify]
  );

  const value = useMemo<StoreValue>(
    () => ({ ready, settings, appInfo, updateSettings, toasts, notify, dismiss }),
    [ready, settings, appInfo, updateSettings, toasts, notify, dismiss]
  );

  return (
    <StoreContext.Provider value={value}>
      <I18nProvider lang={settings.language}>{children}</I18nProvider>
    </StoreContext.Provider>
  );
}

export function useStore(): StoreValue {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error("useStore must be used within AppProvider");
  return ctx;
}
