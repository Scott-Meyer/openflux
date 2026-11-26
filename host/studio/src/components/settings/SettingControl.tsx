import type React from "react";
import type { MediaLibraryFile, SettingField } from "../../api/v2Client";

/**
 * The input for one scalar module setting (`number`, `boolean`, `string`, `color`, `select`, `file`,
 * `key`). It only renders the control; labels, descriptions and saving belong to the caller, which
 * receives every edit through `onChange`. List settings are rendered by `ListSettingEditor`, which
 * uses this control for each row field.
 */
export interface SettingControlProps {
  field: SettingField;
  /** The current stored value; `undefined` shows the field's default. */
  value: unknown;
  onChange: (value: unknown) => void;
  /** DOM id for the control, so a caller's `<label htmlFor>` can name it. */
  id?: string;
  /** Accessible name when the caller has no visible `<label htmlFor>` for this control. */
  ariaLabel?: string;
  /** Media library entries offered for `file` settings. */
  mediaFiles?: MediaLibraryFile[];
  /** Native file picker for `file` settings; the Browse button is hidden without it. */
  pickFile?: (title: string) => Promise<string | null>;
}

export const KEY_INDEX_OPTIONS: Array<{ value: number; label: string }> = [
  { value: 0, label: "Key 0 (Esc)" },
  { value: 12, label: "Key 12 (F12)" },
  { value: 13, label: "Key 13 (PrtSc)" },
  { value: 14, label: "Key 14 (Ins)" },
  { value: 15, label: "Key 15 (Del)" },
  { value: 30, label: "Key 30 (Home)" },
  { value: 45, label: "Key 45 (End)" },
  { value: 59, label: "Key 59 (PgUp)" },
  { value: 73, label: "Key 73 (PgDn)" },
  { value: 80, label: "Key 80 (Bottom-Right 1)" },
  { value: 81, label: "Key 81 (Bottom-Right 2)" },
];

const DEFAULT_COLOR = "#4de1ff";

/**
 * The value a scalar field shows when nothing is stored: its declared default, or a value its
 * control can represent. New list rows are filled with these so what's stored matches what's shown.
 */
export function scalarFallback(field: SettingField): unknown {
  if (field.default !== undefined && field.default !== null) return field.default;
  switch (field.kind) {
    case "boolean":
      return false;
    case "number":
      return field.min ?? 0;
    case "key":
      return 0;
    case "color":
      return DEFAULT_COLOR;
    case "select":
      return field.options?.[0] ?? "";
    default:
      return "";
  }
}

export const SettingControl: React.FC<SettingControlProps> = ({
  field,
  value,
  onChange,
  id,
  ariaLabel,
  mediaFiles = [],
  pickFile,
}) => {
  const cur = value ?? field.default ?? "";
  const a11y = { id, "aria-label": ariaLabel };
  switch (field.kind) {
    case "boolean":
      return <input type="checkbox" {...a11y} checked={Boolean(cur)} onChange={(e) => onChange(e.target.checked)} />;
    case "select":
      return (
        <select {...a11y} value={String(cur)} onChange={(e) => onChange(e.target.value)}>
          {(field.options ?? []).map((opt) => (
            <option key={opt} value={opt}>
              {opt}
            </option>
          ))}
        </select>
      );
    case "number":
      if (field.min !== undefined && field.max !== undefined) {
        return (
          <div className="kg-range-wrap">
            <input
              type="range"
              {...a11y}
              min={field.min}
              max={field.max}
              step={field.max - field.min <= 5 ? 0.05 : 1}
              value={Number(cur) || 0}
              onChange={(e) => onChange(Number(e.target.value))}
            />
            <span className="kg-range-val">{String(cur)}</span>
          </div>
        );
      }
      return (
        <input
          type="number"
          {...a11y}
          min={field.min}
          max={field.max}
          value={Number(cur) || 0}
          onChange={(e) => onChange(Number(e.target.value))}
          style={{ width: 120 }}
        />
      );
    case "color":
      return (
        <input type="color" {...a11y} value={String(cur || DEFAULT_COLOR)} onChange={(e) => onChange(e.target.value)} />
      );
    case "key":
      return (
        <select {...a11y} value={Number(cur) || 0} onChange={(e) => onChange(Number(e.target.value))}>
          {KEY_INDEX_OPTIONS.map((k) => (
            <option key={k.value} value={k.value}>
              {k.label}
            </option>
          ))}
        </select>
      );
    case "file":
      if (mediaFiles.length === 0 && !pickFile) {
        return <input type="text" {...a11y} value={String(cur)} onChange={(e) => onChange(e.target.value)} />;
      }
      return (
        <div className="kg-file-ctrl">
          {mediaFiles.length > 0 && (
            <select {...a11y} value={String(cur)} onChange={(e) => onChange(e.target.value)}>
              <option value="">Choose library media…</option>
              {mediaFiles.map((f) => (
                <option key={f.path} value={f.path}>
                  {f.name}
                </option>
              ))}
            </select>
          )}
          {pickFile && (
            <button
              type="button"
              className="kg-btn-ghost"
              onClick={async () => {
                const p = await pickFile(`Select ${field.label}`);
                if (p) onChange(p);
              }}
            >
              Browse…
            </button>
          )}
        </div>
      );
    default:
      return (
        <input
          type={field.secret ? "password" : "text"}
          {...a11y}
          value={String(cur)}
          onChange={(e) => onChange(e.target.value)}
        />
      );
  }
};
