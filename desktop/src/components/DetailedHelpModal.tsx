import { useMemo, useState } from "react";
import { SECTION_BY_ID, type SectionDef } from "../app/sections";
import { useI18n } from "../i18n";
import { EXPECTED_BY_OP, FIELD_HELP, UI, pick } from "../i18n/help-detail";
import { Modal } from "./ui";

interface FieldLite {
  key: string;
  label: string;
}

function sectionOperation(def: SectionDef): string | null {
  return def.form?.operation ?? def.wizard?.operation ?? def.compare?.operation ?? null;
}

function sectionFields(def: SectionDef): FieldLite[] {
  if (def.form) return def.form.fields;
  if (def.wizard) return def.wizard.steps.flatMap((s) => s.fields);
  if (def.compare) {
    const c = def.compare;
    const fields: FieldLite[] = [
      { key: c.oldKey, label: `fields.${c.oldKey}` },
      { key: c.newKey, label: `fields.${c.newKey}` },
    ];
    if (c.engine) fields.push({ key: "engineHint", label: "fields.engineHint" });
    if (c.extraFields) fields.push(...c.extraFields);
    return fields;
  }
  return [];
}

function createStepKey(def: SectionDef): keyof typeof UI | null {
  if (def.create === "wizard") return "wizardStep";
  if (def.create === "compare") return "compareStep";
  if (def.create === "form") return "createStep";
  return null;
}

export function DetailedHelpModal({
  sectionId,
  onClose,
}: {
  sectionId: string;
  onClose: () => void;
}) {
  const { t, lang, section } = useI18n();
  const def = SECTION_BY_ID[sectionId];
  const text = section(sectionId);
  const op = sectionOperation(def);
  const fields = sectionFields(def);
  const stepKey = createStepKey(def);

  const topics = useMemo(() => {
    const list: { id: string; label: string }[] = [
      { id: "purpose", label: pick(UI.purpose, lang) },
      { id: "howToUse", label: pick(UI.howToUse, lang) },
    ];
    if (op) list.push({ id: "expected", label: pick(UI.expected, lang) });
    if (fields.length > 0) list.push({ id: "fields", label: pick(UI.fields, lang) });
    return list;
  }, [lang, op, fields.length]);

  const [active, setActive] = useState(topics[0].id);

  return (
    <Modal
      title={`${text.label} — ${pick(UI.detailTitle, lang)}`}
      onClose={onClose}
      wide
      footer={
        <>
          <span className="spacer" />
          <button className="btn btn-primary" onClick={onClose}>{t("common.close")}</button>
        </>
      }
    >
      <div className="help-modal">
        <nav className="help-modal-nav">
          {topics.map((tp) => (
            <button
              key={tp.id}
              className={"help-nav-item" + (active === tp.id ? " active" : "")}
              onClick={() => setActive(tp.id)}
            >
              {tp.label}
            </button>
          ))}
        </nav>

        <div className="help-modal-content">
          {active === "purpose" && (
            <>
              <h3>{pick(UI.purpose, lang)}</h3>
              <p>{text.help.summary}</p>
              <p className="text-dim">{text.tagline}</p>
            </>
          )}

          {active === "howToUse" && (
            <>
              <h3>{pick(UI.howToUse, lang)}</h3>
              <ol className="help-steps">
                {stepKey && <li>{pick(UI[stepKey], lang)}</li>}
                {text.help.points.map((p, i) => (
                  <li key={i}>{p}</li>
                ))}
                {stepKey && <li>{pick(UI.background, lang)}</li>}
                <li>{pick(UI.historyNote, lang)}</li>
              </ol>
            </>
          )}

          {active === "expected" && op && (
            <>
              <h3>{pick(UI.expected, lang)}</h3>
              <p>{EXPECTED_BY_OP[op] ? pick(EXPECTED_BY_OP[op], lang) : text.help.summary}</p>
            </>
          )}

          {active === "fields" && (
            <>
              <h3>{pick(UI.fields, lang)}</h3>
              {fields.length === 0 ? (
                <p className="text-dim">{pick(UI.noForm, lang)}</p>
              ) : (
                <dl className="help-fields">
                  {fields.map((f) => (
                    <div key={f.key}>
                      <dt>{f.label.includes(".") ? t(f.label) : f.label}</dt>
                      <dd>{FIELD_HELP[f.key] ? pick(FIELD_HELP[f.key], lang) : ""}</dd>
                    </div>
                  ))}
                </dl>
              )}
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}
