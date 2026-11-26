import type React from "react";
import { useEffect, useId, useRef, useState } from "react";
import type { MediaLibraryFile, SettingField } from "../../api/v2Client";
import { SettingControl, scalarFallback } from "./SettingControl";

/**
 * Editor for a `kind: "list"` module setting: a list of rows whose fields are described by
 * `field.fields`. Rows can be added, removed (with a confirm step) and reordered; each row shows a
 * one-line summary and expands to its fields, which use the ordinary scalar `SettingControl`.
 *
 * The editor owns a draft so typing stays responsive while saves go out: every edit reports the
 * whole new array through `onChange` (text and number edits are batched briefly, structural and
 * picker edits immediately, and anything pending is sent on unmount). A changed `value` from
 * outside (another view, `fluxctl`) replaces the draft unless the user has an unsent edit.
 *
 * Row objects are only ever spread and patched, so keys the manifest doesn't declare (a row `id`,
 * fields from a newer module version) survive editing. Entries that aren't objects are kept, shown
 * as unrecognized, and can still be moved or removed.
 */
export interface ListSettingEditorProps {
  field: SettingField;
  /** The stored value; `undefined` falls back to the field's default. Anything else that isn't an array is treated as empty. */
  value: unknown;
  /** Receives the complete new list after each edit (see batching above). */
  onChange: (rows: unknown[]) => void;
  mediaFiles?: MediaLibraryFile[];
  pickFile?: (title: string) => Promise<string | null>;
  /** How long text and number edits wait for more typing before being reported. */
  commitDelayMs?: number;
}

type Row = Record<string, unknown>;

const TITLE_KEYS = ["label", "name", "title"];
const BATCHED_KINDS = new Set(["string", "number", "file"]);

function isRow(v: unknown): v is Row {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function storedRows(value: unknown, field: SettingField): unknown[] {
  const v = value === undefined || value === null ? field.default : value;
  return Array.isArray(v) ? v : [];
}

/** A new row with every declared field set to the value its control would show. */
export function newListRow(field: SettingField): Row {
  const row: Row = {};
  for (const f of field.fields ?? []) row[f.key] = scalarFallback(f);
  return row;
}

function titleField(fields: SettingField[]): SettingField | undefined {
  const strings = fields.filter((f) => f.kind === "string" && !f.secret);
  return TITLE_KEYS.map((k) => strings.find((f) => f.key === k)).find(Boolean) ?? strings[0];
}

function rowTitle(fields: SettingField[], row: unknown, index: number): string {
  const tf = titleField(fields);
  const t = tf && isRow(row) ? row[tf.key] : undefined;
  return typeof t === "string" && t.trim() ? t.trim() : `Item ${index + 1}`;
}

function RowSummary({ fields, row }: { fields: SettingField[]; row: Row }) {
  const tf = titleField(fields);
  const bits: React.ReactNode[] = [];
  for (const f of fields) {
    if (bits.length >= 3 || f === tf || f.secret) continue;
    const v = row[f.key] ?? scalarFallback(f);
    if (f.kind === "select" && v !== "") {
      bits.push(<span key={f.key}>{String(v)}</span>);
    } else if (f.kind === "color" && typeof v === "string" && v) {
      bits.push(<span key={f.key} className="kg-list-swatch" style={{ background: v }} title={`${f.label}: ${v}`} />);
    } else if (f.kind === "boolean" && v === true) {
      bits.push(<span key={f.key}>{f.label}</span>);
    }
  }
  return bits.length > 0 ? <span className="kg-list-row-chips">{bits}</span> : null;
}

export const ListSettingEditor: React.FC<ListSettingEditorProps> = ({
  field,
  value,
  onChange,
  mediaFiles,
  pickFile,
  commitDelayMs = 400,
}) => {
  const domId = useId();
  const fields = (field.fields ?? []).filter((f) => f.kind !== "list");
  const incoming = storedRows(value, field);
  const incomingJson = JSON.stringify(incoming);
  const notAList = value !== undefined && value !== null && !Array.isArray(value);

  const nextKey = useRef(0);
  const freshKeys = (n: number) => Array.from({ length: n }, () => nextKey.current++);

  const [draft, setDraft] = useState<unknown[]>(incoming);
  const [rowKeys, setRowKeys] = useState<number[]>(() => freshKeys(incoming.length));
  const [open, setOpen] = useState<Set<number>>(() => new Set());
  const [confirmRemove, setConfirmRemove] = useState<number | null>(null);

  const draftRef = useRef(draft);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unsent = useRef<unknown[] | null>(null);
  /** JSON of lists we've reported; a stored value matching an older one is our own write echoing back. */
  const sent = useRef<string[]>([]);

  const flush = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const rows = unsent.current;
    if (!rows) return;
    unsent.current = null;
    sent.current = [...sent.current.slice(-9), JSON.stringify(rows)];
    onChangeRef.current(rows);
  };

  // Adopt outside changes, unless the user has an edit that hasn't been reported yet.
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed on the serialized stored value only
  useEffect(() => {
    if (unsent.current) return;
    if (incomingJson === JSON.stringify(draftRef.current)) {
      sent.current = [];
      return;
    }
    if (sent.current.includes(incomingJson)) return;
    const next = JSON.parse(incomingJson) as unknown[];
    draftRef.current = next;
    setDraft(next);
    setRowKeys((keys) => (keys.length === next.length ? keys : freshKeys(next.length)));
    setConfirmRemove(null);
  }, [incomingJson]);

  // Send anything pending when the editor goes away (switching modules, leaving the view).
  // biome-ignore lint/correctness/useExhaustiveDependencies: flush reads refs only
  useEffect(() => () => flush(), []);

  const update = (next: unknown[], keys: number[], batched: boolean) => {
    draftRef.current = next;
    setDraft(next);
    setRowKeys(keys);
    unsent.current = next;
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (batched) timer.current = setTimeout(flush, commitDelayMs);
    else flush();
  };

  const setRowField = (index: number, f: SettingField, v: unknown) => {
    const row = draft[index];
    if (!isRow(row)) return;
    const next = draft.slice();
    next[index] = { ...row, [f.key]: v };
    update(next, rowKeys, BATCHED_KINDS.has(f.kind));
  };

  const addRow = () => {
    const [k] = freshKeys(1);
    update([...draft, newListRow(field)], [...rowKeys, k], false);
    setOpen((s) => new Set(s).add(k));
  };

  const removeRow = (index: number) => {
    update(
      draft.filter((_, i) => i !== index),
      rowKeys.filter((_, i) => i !== index),
      false,
    );
    setConfirmRemove(null);
  };

  const moveRow = (index: number, delta: -1 | 1) => {
    const to = index + delta;
    if (to < 0 || to >= draft.length) return;
    const next = draft.slice();
    const keys = rowKeys.slice();
    [next[index], next[to]] = [next[to], next[index]];
    [keys[index], keys[to]] = [keys[to], keys[index]];
    update(next, keys, false);
  };

  const toggle = (k: number) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });

  return (
    <div className="kg-list-editor">
      {notAList && (
        <div className="kg-muted">
          The stored value isn&apos;t a list ({JSON.stringify(value).slice(0, 80)}). Adding an item replaces it.
        </div>
      )}
      {draft.length === 0 && !notAList && <div className="kg-muted">No items yet.</div>}
      {draft.map((row, index) => {
        const k = rowKeys[index];
        const title = rowTitle(fields, row, index);
        const known = isRow(row);
        const isOpen = known && open.has(k);
        const bodyId = `${domId}-row-${k}`;
        return (
          <div key={k} className={`kg-list-row${isOpen ? " is-open" : ""}`}>
            <div className="kg-list-row-head">
              {known ? (
                <button
                  type="button"
                  className="kg-list-row-toggle"
                  aria-expanded={isOpen}
                  aria-controls={bodyId}
                  onClick={() => toggle(k)}
                >
                  <span className="kg-list-row-caret" aria-hidden="true">
                    {isOpen ? "▾" : "▸"}
                  </span>
                  <span className="kg-list-row-title">{title}</span>
                  <RowSummary fields={fields} row={row} />
                </button>
              ) : (
                <span className="kg-list-row-toggle kg-muted" title={JSON.stringify(row)}>
                  Unrecognized entry: {JSON.stringify(row).slice(0, 60)}
                </span>
              )}
              <div className="kg-list-row-actions">
                {confirmRemove === k ? (
                  <>
                    <button
                      type="button"
                      className="kg-btn-ghost kg-list-danger"
                      aria-label={`Confirm removing ${title}`}
                      onClick={() => removeRow(index)}
                    >
                      Remove
                    </button>
                    <button type="button" className="kg-btn-ghost" onClick={() => setConfirmRemove(null)}>
                      Keep
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      type="button"
                      className="kg-btn-ghost kg-list-icon-btn"
                      aria-label={`Move ${title} up`}
                      title="Move up"
                      disabled={index === 0}
                      onClick={() => moveRow(index, -1)}
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      className="kg-btn-ghost kg-list-icon-btn"
                      aria-label={`Move ${title} down`}
                      title="Move down"
                      disabled={index === draft.length - 1}
                      onClick={() => moveRow(index, 1)}
                    >
                      ↓
                    </button>
                    <button
                      type="button"
                      className="kg-btn-ghost kg-list-icon-btn"
                      aria-label={`Remove ${title}`}
                      title="Remove"
                      onClick={() => setConfirmRemove(k)}
                    >
                      ✕
                    </button>
                  </>
                )}
              </div>
            </div>
            {isOpen && (
              <div className="kg-list-row-body" id={bodyId}>
                {fields.map((f) => {
                  const ctrlId = `${bodyId}-${f.key}`;
                  return (
                    <div key={f.key} className="kg-list-field">
                      <div className="kg-setting-meta">
                        <label className="kg-setting-label" htmlFor={ctrlId}>
                          {f.label}
                        </label>
                        {f.description && <div className="kg-muted">{f.description}</div>}
                      </div>
                      <div className="kg-setting-ctrl">
                        <SettingControl
                          field={f}
                          id={ctrlId}
                          value={(row as Row)[f.key]}
                          onChange={(v) => setRowField(index, f, v)}
                          mediaFiles={mediaFiles}
                          pickFile={pickFile}
                        />
                      </div>
                    </div>
                  );
                })}
                {fields.length === 0 && <div className="kg-muted">This list declares no row fields.</div>}
              </div>
            )}
          </div>
        );
      })}
      <div>
        <button type="button" className="kg-btn-ghost" aria-label={`Add ${field.label} item`} onClick={addRow}>
          + Add item
        </button>
      </div>
    </div>
  );
};
