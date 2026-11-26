import type React from "react";
import { useState } from "react";
import type { DescribeContext, RuleRow } from "./rules";
import { describeRule, str } from "./rules";

export interface RuleListProps {
  rows: unknown[];
  openIndex: number | null;
  describe: DescribeContext;
  onOpen: (index: number | null) => void;
  /** `batch` edits (typing) may be held briefly before saving. */
  onChangeRows: (rows: unknown[], batch?: boolean) => void;
  /** The editor for the open row. */
  renderEditor: (row: RuleRow, index: number) => React.ReactNode;
}

function isRow(v: unknown): v is RuleRow {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** The user's rules in order (later rows win), each as a sentence; one opens to edit. */
export const RuleList: React.FC<RuleListProps> = ({
  rows,
  openIndex,
  describe,
  onOpen,
  onChangeRows,
  renderEditor,
}) => {
  const [confirming, setConfirming] = useState<number | null>(null);

  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= rows.length) return;
    const next = [...rows];
    [next[i], next[j]] = [next[j], next[i]];
    onChangeRows(next);
    if (openIndex === i) onOpen(j);
    else if (openIndex === j) onOpen(i);
  };
  const remove = (i: number) => {
    onChangeRows(rows.filter((_, k) => k !== i));
    setConfirming(null);
    if (openIndex === i) onOpen(null);
    else if (openIndex !== null && openIndex > i) onOpen(openIndex - 1);
  };
  const toggle = (i: number, on: boolean) => {
    const r = rows[i];
    if (!isRow(r)) return;
    const next = [...rows];
    next[i] = { ...r, enabled: on };
    onChangeRows(next);
  };

  if (rows.length === 0) {
    return (
      <p className="muted kr-empty">
        No rules yet. Pick keys on the keyboard above, then choose what they should show.
      </p>
    );
  }

  return (
    <ol className="kr-rules" aria-label="Your rules">
      {rows.map((r, i) => {
        const row = isRow(r) ? r : null;
        const name = row ? str(row.name) : "";
        const sentence = row ? describeRule(row, describe) : "Not a rule this version of Studio understands.";
        const title = name || `Rule ${i + 1}`;
        const enabled = row?.enabled !== false;
        const open = openIndex === i;
        return (
          // biome-ignore lint/suspicious/noArrayIndexKey: rows have no ids; position is their identity
          <li key={i} className={`kr-rule${open ? " is-open" : ""}${enabled ? "" : " is-off"}`}>
            <div className="kr-rule-head">
              <input
                type="checkbox"
                aria-label={`Enable ${title}`}
                checked={enabled}
                disabled={!row}
                onChange={(e) => toggle(i, e.target.checked)}
              />
              <button
                type="button"
                className="kr-rule-toggle"
                aria-expanded={open}
                aria-label={`${title}: ${sentence}`}
                onClick={() => onOpen(open ? null : i)}
                disabled={!row}
              >
                <span className="kr-rule-num">{i + 1}</span>
                <span className="kr-rule-text">
                  {name && <strong>{name}. </strong>}
                  {sentence}
                </span>
              </button>
              <span className="kg-list-row-actions">
                <button
                  type="button"
                  className="btn-secondary kg-list-icon-btn"
                  aria-label={`Move ${title} up`}
                  disabled={i === 0}
                  onClick={() => move(i, -1)}
                >
                  ↑
                </button>
                <button
                  type="button"
                  className="btn-secondary kg-list-icon-btn"
                  aria-label={`Move ${title} down`}
                  disabled={i === rows.length - 1}
                  onClick={() => move(i, 1)}
                >
                  ↓
                </button>
                {confirming === i ? (
                  <>
                    <button
                      type="button"
                      className="btn-danger kg-list-icon-btn"
                      aria-label={`Confirm deleting ${title}`}
                      onClick={() => remove(i)}
                    >
                      Delete
                    </button>
                    <button
                      type="button"
                      className="btn-secondary kg-list-icon-btn"
                      onClick={() => setConfirming(null)}
                    >
                      Keep
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    className="btn-secondary kg-list-icon-btn kg-list-danger"
                    aria-label={`Delete ${title}`}
                    onClick={() => setConfirming(i)}
                  >
                    ✕
                  </button>
                )}
              </span>
            </div>
            {open && row && (
              <div className="kr-rule-body">
                <label className="field">
                  <span>Name (optional)</span>
                  <input
                    type="text"
                    value={str(row.name)}
                    placeholder="e.g. Clean home row"
                    onChange={(e) => {
                      const next = [...rows];
                      next[i] = { ...row, name: e.target.value };
                      onChangeRows(next, true);
                    }}
                  />
                </label>
                {renderEditor(row, i)}
              </div>
            )}
          </li>
        );
      })}
    </ol>
  );
};
