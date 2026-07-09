import { api, errMessage } from "../api/client";
import type { OperationRecord } from "../api/types";
import { useI18n } from "../i18n";
import { useStore } from "../app/store";
import { formatDate } from "../lib/format";
import { Icon } from "./Icon";
import { StatusBadge, EmptyState } from "./ui";

export function HistoryTable({
  records,
  onOpen,
  onChanged,
}: {
  records: OperationRecord[];
  onOpen: (rec: OperationRecord) => void;
  onChanged: () => void;
}) {
  const { t, lang } = useI18n();
  const { notify } = useStore();

  const del = async (e: React.MouseEvent, rec: OperationRecord) => {
    e.stopPropagation();
    if (!window.confirm(t("history.confirmDelete"))) return;
    try {
      await api.deleteOperation(rec.id);
      notify("success", t("toast.deleted"));
      onChanged();
    } catch (err) {
      notify("error", `${t("history.deleteFailed")}: ${errMessage(err)}`);
    }
  };

  const openFolder = async (e: React.MouseEvent, rec: OperationRecord) => {
    e.stopPropagation();
    try {
      await api.openPath(rec.artifactDir);
    } catch (err) {
      notify("error", errMessage(err));
    }
  };

  if (records.length === 0) {
    return <EmptyState text={t("history.empty")} />;
  }

  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>{t("history.columns.title")}</th>
            <th>{t("history.columns.date")}</th>
            <th>{t("history.columns.status")}</th>
            <th style={{ textAlign: "center" }}>{t("history.columns.files")}</th>
            <th style={{ textAlign: "right" }}>{t("history.columns.actions")}</th>
          </tr>
        </thead>
        <tbody>
          {records.map((rec) => (
            <tr key={rec.id} className="clickable" onClick={() => onOpen(rec)}>
              <td>
                <div style={{ fontWeight: 600 }}>{rec.title}</div>
                <div className="text-dim mono" style={{ fontSize: 11 }}>{rec.kind}</div>
              </td>
              <td className="text-dim">{formatDate(rec.createdAt, lang)}</td>
              <td><StatusBadge status={rec.status} /></td>
              <td style={{ textAlign: "center" }} className="text-dim">{rec.files.length}</td>
              <td>
                <div className="row-actions">
                  <button
                    className="btn btn-icon btn-ghost"
                    title={t("common.info")}
                    onClick={(e) => { e.stopPropagation(); onOpen(rec); }}
                  >
                    <Icon name="info" size={16} />
                  </button>
                  <button
                    className="btn btn-icon btn-ghost"
                    title={t("common.openFolder")}
                    onClick={(e) => openFolder(e, rec)}
                  >
                    <Icon name="folder-open" size={16} />
                  </button>
                  <button
                    className="btn btn-icon btn-danger"
                    title={t("common.delete")}
                    onClick={(e) => del(e, rec)}
                  >
                    <Icon name="trash" size={16} />
                  </button>
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
