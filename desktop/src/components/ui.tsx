import { useEffect, useState, type ReactNode } from "react";
import { Icon } from "./Icon";
import { useI18n } from "../i18n";
import { useStore } from "../app/store";

// ---------------- Modal ----------------
export function Modal({
  title,
  onClose,
  children,
  footer,
  wide,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div
        className={"modal" + (wide ? " wide" : "")}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2>{title}</h2>
          <button className="btn btn-icon btn-ghost" onClick={onClose} aria-label="Close">
            <Icon name="close" size={18} />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-foot">{footer}</div>}
      </div>
    </div>
  );
}

// ---------------- CodeBlock ----------------
export function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      /* ignore */
    }
  };
  return (
    <div className="code">
      <button className="btn btn-icon btn-ghost code-copy" onClick={copy} title={t("common.copy")}>
        <Icon name={copied ? "check" : "copy"} size={15} />
      </button>
      {lang && <div style={{ color: "var(--text-faint)", fontSize: 10.5, marginBottom: 6 }}>{lang}</div>}
      {code}
    </div>
  );
}

// ---------------- Status badge ----------------
export function StatusBadge({ status }: { status: string }) {
  const { t } = useI18n();
  if (status === "completed")
    return (
      <span className="badge green">
        <span className="dot green" /> {t("common.completed")}
      </span>
    );
  return (
    <span className="badge red">
      <span className="dot red" /> {t("common.failed")}
    </span>
  );
}

// ---------------- Empty state ----------------
export function EmptyState({ text }: { text: string }) {
  return (
    <div className="empty">
      <div>{text}</div>
    </div>
  );
}

// ---------------- Toasts ----------------
export function Toasts() {
  const { toasts, dismiss } = useStore();
  return (
    <div className="toasts">
      {toasts.map((toast) => (
        <div key={toast.id} className={"toast " + toast.kind} onClick={() => dismiss(toast.id)}>
          <span className="tbar" />
          <span>{toast.message}</span>
        </div>
      ))}
    </div>
  );
}
