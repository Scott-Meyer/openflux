import type React from "react";
import { useState } from "react";
import type { KeyglassConfig } from "../types/keyglass";
import type { ConfigEdit } from "./layerModeEdits";

type AcceptedEdit = Extract<ConfigEdit, { ok: true }>;

/**
 * Runs layer and actuation-mode edits: an edit without side effects is applied at once, one with
 * side effects waits for the user to confirm them, and a refused edit shows why.
 * `dialog` must be rendered for confirmations to appear.
 */
export function useConfigEdits(onApply: (patch: Partial<KeyglassConfig>) => void) {
  const [pending, setPending] = useState<{ title: string; edit: AcceptedEdit; after?: () => void } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  /** Runs `edit`, titled `title` in its confirmation; `after` runs once it is applied. */
  const request = (title: string, edit: ConfigEdit, after?: () => void) => {
    if (!edit.ok) {
      setNotice(edit.reason);
      return;
    }
    if (edit.consequences.length > 0) {
      setNotice(null);
      setPending({ title, edit, after });
      return;
    }
    onApply(edit.patch);
    after?.();
    setNotice(edit.summary);
  };

  const dialog = pending ? (
    <div className="modal-backdrop" role="dialog" aria-label={pending.title}>
      <div className="modal-card">
        <h3>{pending.title}</h3>
        <ul>
          {pending.edit.consequences.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>
        <div className="btn-group">
          <button type="button" className="btn-secondary" onClick={() => setPending(null)}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => {
              onApply(pending.edit.patch);
              pending.after?.();
              setNotice(pending.edit.summary);
              setPending(null);
            }}
          >
            {pending.title}
          </button>
        </div>
      </div>
    </div>
  ) : null;

  return { request, dialog, notice };
}

/** A name field with a submit button; submitting passes the name as typed and clears the field. */
export const NameForm: React.FC<{
  label: string;
  submitLabel: string;
  initial?: string;
  disabled?: boolean;
  onSubmit: (name: string) => void;
  onCancel?: () => void;
}> = ({ label, submitLabel, initial = "", disabled = false, onSubmit, onCancel }) => {
  const [name, setName] = useState(initial);
  return (
    <form
      className="btn-group"
      onSubmit={(e) => {
        e.preventDefault();
        if (disabled) return;
        onSubmit(name);
        setName("");
      }}
    >
      <input
        type="text"
        aria-label={label}
        placeholder={label}
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      <button type="submit" className="btn-secondary" disabled={disabled}>
        {submitLabel}
      </button>
      {onCancel && (
        <button type="button" className="btn-secondary" onClick={onCancel}>
          Cancel
        </button>
      )}
    </form>
  );
};
