import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { pickPath } from "../api/client";
import type { FieldDef, SectionDef } from "../app/sections";
import { ENGINE } from "../app/sections";
import { useI18n } from "../i18n";
import { useActivities } from "../app/activities";
import { useProjects } from "../app/projects";
import { basename } from "../lib/format";
import { Modal } from "./ui";
import { FieldRenderer, initialValues, missingRequired } from "./fields";

export function CreateModal({
  section,
  onClose,
}: {
  section: SectionDef;
  onClose: () => void;
}) {
  if (section.create === "wizard" && section.wizard)
    return <WizardModal section={section} onClose={onClose} />;
  if (section.create === "compare" && section.compare)
    return <CompareModal section={section} onClose={onClose} />;
  if (section.create === "form" && section.form)
    return <FormModal section={section} onClose={onClose} />;
  return null;
}

// Fire the operation into the background and close the modal immediately.
function useStarter(section: SectionDef, onClose: () => void) {
  const { start } = useActivities();
  return (operation: string, title: string, params: Record<string, any>) => {
    start({ section: section.id, kind: operation, title, params });
    onClose();
  };
}

// ---------------- Form ----------------
function FormModal({ section, onClose }: ModalProps) {
  const { t, section: st } = useI18n();
  const cfg = section.form!;
  const [values, setValues] = useState<Record<string, any>>(() => initialValues(cfg.fields));
  const run = useStarter(section, onClose);

  const submit = () => run(cfg.operation, deriveTitle(cfg.fields, values), values);

  return (
    <Modal
      title={st(section.id).label}
      onClose={onClose}
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button
            className="btn btn-primary"
            disabled={missingRequired(cfg.fields, values)}
            onClick={submit}
          >
            {t("common.run")}
          </button>
        </>
      }
    >
      {cfg.fields.map((f) => (
        <FieldRenderer
          key={f.key}
          field={f}
          value={values[f.key]}
          onChange={(v) => setValues((s) => ({ ...s, [f.key]: v }))}
        />
      ))}
    </Modal>
  );
}

// ---------------- Wizard ----------------
function WizardModal({ section, onClose }: ModalProps) {
  const { t, section: st } = useI18n();
  const cfg = section.wizard!;
  const sText = st(section.id);
  const allFields = cfg.steps.flatMap((s) => s.fields);
  const [values, setValues] = useState<Record<string, any>>(() => initialValues(allFields));
  const [step, setStep] = useState(0);
  const run = useStarter(section, onClose);

  const current = cfg.steps[step];
  const isLast = step === cfg.steps.length - 1;
  const stepTitle = sText.steps?.[current.title] ?? current.title;
  const stepIncomplete = missingRequired(current.fields, values);

  const finish = () => run(cfg.operation, deriveTitle(allFields, values), values);

  return (
    <Modal
      title={st(section.id).label}
      onClose={onClose}
      wide
      footer={
        <>
          <button
            className="btn"
            disabled={step === 0}
            onClick={() => setStep((s) => Math.max(0, s - 1))}
          >
            {t("common.back")}
          </button>
          <span className="spacer" />
          <span className="text-dim" style={{ fontSize: 12 }}>
            {t("common.step")} {step + 1} {t("common.of")} {cfg.steps.length}
          </span>
          {!isLast ? (
            <button
              className="btn btn-primary"
              disabled={stepIncomplete}
              onClick={() => setStep((s) => Math.min(cfg.steps.length - 1, s + 1))}
            >
              {t("common.next")}
            </button>
          ) : (
            <button className="btn btn-primary" disabled={stepIncomplete} onClick={finish}>
              {t("common.finish")}
            </button>
          )}
        </>
      }
    >
      <div className="stepper">
        {cfg.steps.map((s, i) => (
          <div key={s.id} style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <div className={"step-chip " + (i === step ? "active" : i < step ? "done" : "")}>
              <span className="num">{i < step ? "✓" : i + 1}</span>
              {sText.steps?.[s.title] ?? s.title}
            </div>
            {i < cfg.steps.length - 1 && <span className="step-sep" />}
          </div>
        ))}
      </div>

      <h3 style={{ marginTop: 0 }}>{stepTitle}</h3>
      {current.fields.map((f) => (
        <FieldRenderer
          key={f.key}
          field={f}
          value={values[f.key]}
          onChange={(v) => setValues((s) => ({ ...s, [f.key]: v }))}
        />
      ))}
    </Modal>
  );
}

// ---------------- Compare ----------------
function CompareModal({ section, onClose }: ModalProps) {
  const { t, section: st } = useI18n();
  const cfg = section.compare!;
  const { currentProject } = useProjects();
  const projectEngine = currentProject?.engine ?? "auto";
  // A section-specific default (e.g. "godot" for the PCK analyzer) wins;
  // otherwise inherit the project's engine so the UI matches the project.
  const defaultEngine =
    cfg.engineDefault && cfg.engineDefault !== "auto"
      ? cfg.engineDefault
      : ENGINE.some((o) => o.value === projectEngine)
      ? projectEngine
      : "auto";
  const [oldPath, setOldPath] = useState("");
  const [newPath, setNewPath] = useState("");
  const [engine, setEngine] = useState(defaultEngine);
  const [extra, setExtra] = useState<Record<string, any>>(() =>
    initialValues(cfg.extraFields ?? [])
  );
  const run = useStarter(section, onClose);

  // Native Tauri drag-and-drop: fill old first, then new.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "drop") {
          const paths = event.payload.paths;
          if (paths.length >= 1 && !oldPath) setOldPath(paths[0]);
          if (paths.length >= 2) setNewPath(paths[1]);
          else if (paths.length >= 1 && oldPath && !newPath) setNewPath(paths[0]);
        }
      })
      .then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, [oldPath, newPath]);

  const submit = () => {
    const params: Record<string, any> = {
      [cfg.oldKey]: oldPath,
      [cfg.newKey]: newPath,
      ...extra,
    };
    if (cfg.engine) params.engineHint = engine;
    const title = `${basename(oldPath)} → ${basename(newPath)}`;
    run(cfg.operation, title, params);
  };

  return (
    <Modal
      title={st(section.id).label}
      onClose={onClose}
      wide
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button className="btn btn-primary" disabled={!oldPath || !newPath} onClick={submit}>
            {t("compare.action")}
          </button>
        </>
      }
    >
      <div className="compare-cols">
        <DropZone
          label={t("compare.dropOld")}
          hint={cfg.oldType === "folder" ? t("compare.hintFolder") : t("compare.hintFile")}
          path={oldPath}
          directory={cfg.oldType === "folder"}
          onPick={setOldPath}
        />
        <DropZone
          label={t("compare.dropNew")}
          hint={cfg.newType === "folder" ? t("compare.hintFolder") : t("compare.hintFile")}
          path={newPath}
          directory={cfg.newType === "folder"}
          onPick={setNewPath}
        />
      </div>

      {cfg.engine && (
        <div className="field" style={{ marginTop: 16 }}>
          <label>{t("compare.engine")}</label>
          <select className="select" value={engine} onChange={(e) => setEngine(e.target.value)}>
            {ENGINE.map((o) => (
              <option key={o.value} value={o.value}>{o.label}</option>
            ))}
          </select>
        </div>
      )}

      {(cfg.extraFields ?? []).map((f) => (
        <FieldRenderer
          key={f.key}
          field={f}
          value={extra[f.key]}
          onChange={(v) => setExtra((s) => ({ ...s, [f.key]: v }))}
        />
      ))}
    </Modal>
  );
}

function DropZone({
  label,
  hint,
  path,
  directory,
  onPick,
}: {
  label: string;
  hint: string;
  path: string;
  directory: boolean;
  onPick: (p: string) => void;
}) {
  const browse = async () => {
    const picked = await pickPath({ directory, title: label });
    if (picked) onPick(picked);
  };
  return (
    <div className="dropzone" onClick={browse}>
      <div className="dz-label">{label}</div>
      <div style={{ marginTop: 6 }}>{hint}</div>
      {path && <div className="dz-path">{path}</div>}
    </div>
  );
}

function deriveTitle(fields: FieldDef[], values: Record<string, any>): string {
  const outKey = ["outputCavs", "outputPlan", "outputPath", "assetName", "inputDir", "target", "oldPath"].find(
    (k) => fields.some((f) => f.key === k) && values[k]
  );
  const v = outKey ? values[outKey] : undefined;
  const version = values.newVersion ? ` ${values.newVersion}` : "";
  return v ? basename(String(v)) + version : new Date().toISOString().slice(0, 16);
}

interface ModalProps {
  section: SectionDef;
  onClose: () => void;
}
