import { useState } from "react";
import type { OperationRecord } from "../api/types";
import type { SectionDef } from "../app/sections";
import { useI18n } from "../i18n";
import { useOperations } from "../hooks/useOperations";
import { HelpPanel } from "../components/HelpPanel";
import { HistoryTable } from "../components/HistoryTable";
import { CreateModal } from "../components/CreateModal";
import { ResultView } from "../components/ResultView";
import { Modal } from "../components/ui";
import { Icon } from "../components/Icon";

export function SectionPage({ section }: { section: SectionDef }) {
  const { t, section: st } = useI18n();
  const { records, refresh } = useOperations(section.id);
  const [creating, setCreating] = useState(false);
  const [viewing, setViewing] = useState<OperationRecord | null>(null);
  const text = st(section.id);

  const canCreate = section.create !== "none" && section.create !== "custom";

  return (
    <div className="content-inner">
      <div className="page-head">
        <div>
          <h1 className="page-title">{text.label}</h1>
          <p className="page-tagline">{text.tagline}</p>
        </div>
        {canCreate && (
          <div className="section-actions">
            <button className="btn btn-primary btn-lg" onClick={() => setCreating(true)}>
              <Icon name="plus" size={17} />
              {t("common.create")}
            </button>
          </div>
        )}
      </div>

      <HelpPanel sectionId={section.id} />

      <div className="subhead" style={{ marginTop: 8 }}>{t("history.title")}</div>
      <HistoryTable
        records={records}
        onOpen={(rec) => setViewing(rec)}
        onChanged={refresh}
      />

      {creating && (
        <CreateModal section={section} onClose={() => setCreating(false)} />
      )}

      {viewing && (
        <Modal title={`${text.label} — ${viewing.title}`} onClose={() => setViewing(null)} wide>
          <ResultView record={viewing} />
        </Modal>
      )}
    </div>
  );
}
