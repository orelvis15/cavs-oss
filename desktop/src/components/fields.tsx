import { pickPath } from "../api/client";
import type { FieldDef } from "../app/sections";
import { useI18n } from "../i18n";
import { Icon } from "./Icon";

function labelFor(t: (p: string) => string, label: string): string {
  return label.includes(".") ? t(label) : label;
}

export function FieldRenderer({
  field,
  value,
  onChange,
}: {
  field: FieldDef;
  value: any;
  onChange: (v: any) => void;
}) {
  const { t } = useI18n();
  const label = labelFor(t, field.label);

  if (field.type === "file" || field.type === "folder") {
    const browse = async () => {
      const picked = await pickPath({
        directory: field.type === "folder",
        title: label,
      });
      if (picked) onChange(picked);
    };
    return (
      <div className="field">
        <label>
          {label}{" "}
          {field.optional && <span className="hint">({t("common.optional")})</span>}
        </label>
        <div className="file-input">
          <input
            className="input mono"
            value={value ?? ""}
            placeholder={field.type === "folder" ? t("common.selectFolder") : t("common.selectFile")}
            onChange={(e) => onChange(e.target.value)}
          />
          <button type="button" className="btn" onClick={browse}>
            <Icon name="folder-open" size={15} />
            {t("common.browse")}
          </button>
        </div>
      </div>
    );
  }

  if (field.type === "select") {
    return (
      <div className="field">
        <label>{label}</label>
        <select className="select" value={value ?? ""} onChange={(e) => onChange(e.target.value)}>
          {field.options?.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </div>
    );
  }

  return (
    <div className="field">
      <label>
        {label}{" "}
        {field.optional && <span className="hint">({t("common.optional")})</span>}
      </label>
      <input
        className="input"
        type={field.type === "number" ? "number" : "text"}
        value={value ?? ""}
        placeholder={field.placeholder}
        onChange={(e) =>
          onChange(field.type === "number" ? Number(e.target.value) : e.target.value)
        }
      />
    </div>
  );
}

export function initialValues(fields: FieldDef[]): Record<string, any> {
  const out: Record<string, any> = {};
  for (const f of fields) {
    if (f.default !== undefined) out[f.key] = f.default;
  }
  return out;
}

export function missingRequired(fields: FieldDef[], values: Record<string, any>): boolean {
  return fields.some(
    (f) => !f.optional && (values[f.key] === undefined || values[f.key] === "" || values[f.key] === null)
  );
}
