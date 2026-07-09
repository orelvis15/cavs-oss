import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { UI, pick } from "../i18n/help-detail";
import { DetailedHelpModal } from "./DetailedHelpModal";

export function HelpPanel({ sectionId }: { sectionId: string }) {
  const { t, lang, section } = useI18n();
  const seenKey = `cavs-help-seen-${sectionId}`;
  // Open the first time a section is visited; collapsed afterwards (the
  // "show help" button reveals it again).
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(seenKey) !== "1";
    } catch {
      return true;
    }
  });
  const [detail, setDetail] = useState(false);
  const s = section(sectionId);

  useEffect(() => {
    try {
      localStorage.setItem(seenKey, "1");
    } catch {
      /* ignore */
    }
  }, [seenKey]);

  return (
    <div className="help">
      <div className="help-head" style={{ justifyContent: "space-between", display: "flex" }}>
        <span
          style={{ cursor: "pointer" }}
          onClick={() => setOpen((o) => !o)}
        >
          {t("help.title")}
        </span>
        <span className="row" style={{ gap: 12 }}>
          <button
            className="help-more"
            onClick={(e) => {
              e.stopPropagation();
              setDetail(true);
            }}
          >
            {pick(UI.seeMore, lang)}
          </button>
          <span
            className="text-dim"
            style={{ fontSize: 12, fontWeight: 500, cursor: "pointer" }}
            onClick={() => setOpen((o) => !o)}
          >
            {open ? t("help.hide") : t("help.show")}
          </span>
        </span>
      </div>
      {open && (
        <>
          <p>{s.help.summary}</p>
          <ul>
            {s.help.points.map((p, i) => (
              <li key={i}>{p}</li>
            ))}
          </ul>
        </>
      )}

      {detail && <DetailedHelpModal sectionId={sectionId} onClose={() => setDetail(false)} />}
    </div>
  );
}
