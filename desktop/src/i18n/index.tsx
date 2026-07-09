import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  type ReactNode,
} from "react";
import type { Lang } from "../api/types";
import { en, type Dict } from "./en";
import { es } from "./es";
import { SECTION_TEXT, type SectionText } from "./sections";

const DICTS: Record<Lang, Dict> = { en, es };

interface I18nValue {
  lang: Lang;
  t: (path: string) => string;
  section: (id: string) => SectionText;
  group: (name: string) => string;
}

const I18nContext = createContext<I18nValue | null>(null);

function resolve(dict: any, path: string): string {
  const parts = path.split(".");
  let cur: any = dict;
  for (const p of parts) {
    if (cur && typeof cur === "object" && p in cur) cur = cur[p];
    else return path; // fall back to the key so missing strings are visible
  }
  return typeof cur === "string" ? cur : path;
}

export function I18nProvider({
  lang,
  children,
}: {
  lang: Lang;
  children: ReactNode;
}) {
  const dict = DICTS[lang] ?? en;

  const t = useCallback((path: string) => resolve(dict, path), [dict]);

  const section = useCallback(
    (id: string): SectionText => {
      const bi = SECTION_TEXT[id];
      if (!bi)
        return {
          label: id,
          tagline: "",
          help: { summary: "", points: [] },
        };
      return bi[lang] ?? bi.en;
    },
    [lang]
  );

  const group = useCallback(
    (name: string) => resolve(dict, `nav.groups.${name}`),
    [dict]
  );

  const value = useMemo(
    () => ({ lang, t, section, group }),
    [lang, t, section, group]
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n must be used within I18nProvider");
  return ctx;
}
