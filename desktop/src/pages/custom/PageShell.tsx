import type { ReactNode } from "react";
import { useI18n } from "../../i18n";
import { HelpPanel } from "../../components/HelpPanel";

export function PageShell({
  sectionId,
  actions,
  children,
}: {
  sectionId: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  const { section } = useI18n();
  const text = section(sectionId);
  return (
    <div className="content-inner">
      <div className="page-head">
        <div>
          <h1 className="page-title">{text.label}</h1>
          <p className="page-tagline">{text.tagline}</p>
        </div>
        {actions && <div className="section-actions">{actions}</div>}
      </div>
      <HelpPanel sectionId={sectionId} />
      {children}
    </div>
  );
}
