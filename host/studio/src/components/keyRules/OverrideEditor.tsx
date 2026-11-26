import type React from "react";
import { useEffect, useState } from "react";
import { v2Client } from "../../api/v2Client";
import { BUILTIN_ICONS } from "./RuleEditor";
import type { CustomOverride } from "./rules";

export interface OverrideAction {
  action_id: string;
  label?: string;
}

export interface OverrideEditorProps {
  keyIndex: number;
  keyName: string;
  activeLayer: number;
  /** The module action bound to this key, if any (overrides can follow the action). */
  action?: OverrideAction;
  byKey: Record<string, CustomOverride>;
  byAction: Record<string, CustomOverride>;
  /** Saves both override maps; the caller stores them as override rows in the rules list. */
  onSave: (
    byKey: Record<string, CustomOverride>,
    byAction: Record<string, CustomOverride>,
    message: string,
  ) => Promise<void>;
  saving: boolean;
  pickFile?: (title: string) => Promise<string | null>;
}

type Scope = "action" | "key_layer" | "key_all";

/**
 * The per-key / per-action override: the quick way to change one key. It's stored as a row in
 * the rules list (marked `override`), so it's visible and orderable like any rule.
 */
export const OverrideEditor: React.FC<OverrideEditorProps> = ({
  keyIndex,
  keyName,
  activeLayer,
  action,
  byKey,
  byAction,
  onSave,
  saving,
  pickFile,
}) => {
  const [scope, setScope] = useState<Scope>("key_layer");
  const [draft, setDraft] = useState<CustomOverride>({});
  const [note, setNote] = useState<string | null>(null);

  const stored = (s: Scope): CustomOverride | undefined =>
    s === "action"
      ? action
        ? byAction[action.action_id]
        : undefined
      : byKey[s === "key_all" ? `*:${keyIndex}` : `${activeLayer}:${keyIndex}`];

  // A new key (or layer, or binding) loads whichever override applies to it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reload only when the key changes, not on every poll
  useEffect(() => {
    const first: Scope =
      action && byAction[action.action_id]
        ? "action"
        : byKey[`${activeLayer}:${keyIndex}`]
          ? "key_layer"
          : byKey[`*:${keyIndex}`]
            ? "key_all"
            : action
              ? "action"
              : "key_layer";
    setScope(first);
    setDraft({ ...(stored(first) ?? {}) });
    setNote(null);
  }, [keyIndex, activeLayer, action?.action_id]);

  const pickScope = (s: Scope) => {
    setScope(s);
    setDraft({ ...(stored(s) ?? {}) });
  };

  const write = async (ov: CustomOverride | null, message: string) => {
    const nextByKey = { ...byKey };
    const nextByAction = { ...byAction };
    if (scope === "action" && action) {
      if (ov) nextByAction[action.action_id] = ov;
      else delete nextByAction[action.action_id];
    } else {
      const k = scope === "key_all" ? `*:${keyIndex}` : `${activeLayer}:${keyIndex}`;
      if (ov) nextByKey[k] = ov;
      else delete nextByKey[k];
    }
    await onSave(nextByKey, nextByAction, message);
  };

  const apply = async () => {
    const c: CustomOverride = {};
    for (const f of ["label", "color", "bg", "border", "icon", "image"] as const) {
      const v = draft[f]?.trim();
      if (v) c[f] = v;
    }
    if (draft.hide) c.hide = true;
    const empty = Object.keys(c).length === 0;
    const what = scope === "action" && action ? `the ${action.label ?? action.action_id} action` : keyName;
    await write(empty ? null : c, empty ? `Removed the override for ${what}.` : `Saved the override for ${what}.`);
  };

  const reset = async () => {
    setDraft({});
    await write(null, "Removed the override; the key shows its rules and keymap again.");
  };

  const pickImage = async () => {
    if (!pickFile) return;
    const local = await pickFile("Choose a PNG for this key");
    if (!local) return;
    try {
      const res = await v2Client.importMediaFile(local);
      setDraft((p) => ({ ...p, image: res.name, icon: "" }));
      setNote(`Imported "${res.name}". Apply to show it on the key.`);
    } catch (err) {
      setNote(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <section className="card kr-override" aria-label="Quick override">
      <h3>Quick override for {keyName}</h3>
      <small className="muted">
        The quick way to change just this key. It's saved as a rule in your list (added at the end, so it wins), and it
        edits whatever the key shows, so layer-tap and modifier markers stay.
      </small>
      <label className="field" style={{ marginTop: 10 }}>
        <span>Applies to</span>
        <select value={scope} onChange={(e) => pickScope(e.target.value as Scope)}>
          {action && <option value="action">The {action.label || action.action_id} action, wherever it's bound</option>}
          <option value="key_layer">
            {keyName} on layer {activeLayer}
          </option>
          <option value="key_all">{keyName} on every layer</option>
        </select>
      </label>
      <label className="field">
        <span>Label</span>
        <input
          type="text"
          placeholder="Keep the current label"
          value={draft.label ?? ""}
          onChange={(e) => setDraft((p) => ({ ...p, label: e.target.value }))}
        />
      </label>
      <div className="row-2">
        <label className="field">
          <span>Icon</span>
          <select
            value={draft.icon ?? ""}
            onChange={(e) => setDraft((p) => ({ ...p, icon: e.target.value, image: e.target.value ? "" : p.image }))}
          >
            <option value="">Keep / none</option>
            {BUILTIN_ICONS.map((ic) => (
              <option key={ic} value={ic}>
                {ic}
              </option>
            ))}
          </select>
        </label>
        <div className="field">
          <span>PNG image</span>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              type="button"
              className="btn-secondary"
              style={{ flex: 1 }}
              onClick={() => void pickImage()}
              disabled={!pickFile}
            >
              {draft.image ? draft.image : "Choose PNG…"}
            </button>
            {draft.image && (
              <button
                type="button"
                className="btn-secondary"
                aria-label="Remove image"
                onClick={() => setDraft((p) => ({ ...p, image: "" }))}
              >
                ✕
              </button>
            )}
          </div>
        </div>
      </div>
      <div className="row-3">
        <label className="field">
          <span>Legend color</span>
          <input
            type="text"
            placeholder="#ffffff"
            value={draft.color ?? ""}
            onChange={(e) => setDraft((p) => ({ ...p, color: e.target.value }))}
          />
        </label>
        <label className="field">
          <span>Backdrop (inset tint)</span>
          <input
            type="text"
            placeholder="#1a2436"
            value={draft.bg ?? ""}
            onChange={(e) => setDraft((p) => ({ ...p, bg: e.target.value }))}
          />
        </label>
        <label className="field">
          <span>Outline (inset window)</span>
          <input
            type="text"
            placeholder="#4de1ff"
            value={draft.border ?? ""}
            onChange={(e) => setDraft((p) => ({ ...p, border: e.target.value }))}
          />
        </label>
      </div>
      <label className="checkbox-inline" style={{ marginBottom: 10 }}>
        <input
          type="checkbox"
          checked={Boolean(draft.hide)}
          onChange={(e) => setDraft((p) => ({ ...p, hide: e.target.checked }))}
        />
        Hide the legend (layer and modifier markers stay)
      </label>
      <div style={{ display: "flex", gap: 8 }}>
        <button
          type="button"
          className="btn-primary"
          style={{ flex: 1 }}
          disabled={saving}
          onClick={() => void apply()}
        >
          {saving ? "Saving…" : "Apply override"}
        </button>
        <button
          type="button"
          className="btn-secondary"
          disabled={saving || !stored(scope)}
          onClick={() => void reset()}
        >
          Remove
        </button>
      </div>
      {note && <small className="muted">{note}</small>}
    </section>
  );
};
