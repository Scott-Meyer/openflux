import type React from "react";
import { useEffect, useRef, useState } from "react";
import type { MacroDefinition, MacroStep } from "../types/openflux";
import { domCodeToQmk } from "./KeycodePickerModal";

export function domCodeToMacroQmk(code: string): string | null {
  const modMap: Record<string, string> = {
    ControlLeft: "KC_LCTL",
    ControlRight: "KC_RCTL",
    ShiftLeft: "KC_LSHIFT",
    ShiftRight: "KC_RSHIFT",
    AltLeft: "KC_LALT",
    AltRight: "KC_RALT",
    MetaLeft: "KC_LGUI",
    MetaRight: "KC_RGUI",
  };
  return modMap[code] ?? domCodeToQmk(code);
}

interface MacroEditorPanelProps {
  macros: MacroDefinition[];
  onChangeMacros: (next: MacroDefinition[]) => void;
  onDeleteMacro: (deletedIdx: number) => void;
  onAssignMacroToSelectedKey: (macroId: number) => void;
}

export const MacroEditorPanel: React.FC<MacroEditorPanelProps> = ({
  macros,
  onChangeMacros,
  onDeleteMacro,
  onAssignMacroToSelectedKey,
}) => {
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [recording, setRecording] = useState(false);
  const lastEventAtRef = useRef<number>(0);
  const macrosRef = useRef<MacroDefinition[]>(macros);
  const selectedIdxRef = useRef<number>(selectedIdx);
  const onChangeRef = useRef(onChangeMacros);

  macrosRef.current = macros;
  selectedIdxRef.current = selectedIdx;
  onChangeRef.current = onChangeMacros;

  const cur = macros[selectedIdx];

  const updateCur = (patch: Partial<MacroDefinition>) => {
    const next = macros.map((m, i) => (i === selectedIdx ? { ...m, ...patch } : m));
    onChangeMacros(next);
  };

  useEffect(() => {
    if (!recording) return;
    lastEventAtRef.current = performance.now();
    const handle = (isDown: boolean) => (e: KeyboardEvent) => {
      if (isDown && e.repeat) return;
      const qmk = domCodeToMacroQmk(e.code);
      if (!qmk) return;
      e.preventDefault();
      e.stopPropagation();
      const now = performance.now();
      const delayMs = Math.min(2000, Math.max(5, Math.round(now - lastEventAtRef.current)));
      lastEventAtRef.current = now;
      const step: MacroStep = { keyCode: qmk, delayMs, isDown };
      const idx = selectedIdxRef.current;
      const next = macrosRef.current.map((m, i) =>
        i === idx && !m.words ? { ...m, steps: [...(m.steps ?? []), step] } : m,
      );
      macrosRef.current = next;
      onChangeRef.current(next);
    };
    const onDown = handle(true);
    const onUp = handle(false);
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    return () => {
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
    };
  }, [recording]);

  return (
    <div className="card" style={{ marginTop: 14 }}>
      <div className="toolbar-between">
        <h3>Macro Recorder &amp; Sequence Editor ({macros.length})</h3>
        <div className="btn-group">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => {
              const next = [
                ...macros,
                {
                  name: `Macro ${macros.length + 1}`,
                  steps: [],
                },
              ];
              onChangeMacros(next);
              setSelectedIdx(next.length - 1);
            }}
          >
            + New Macro
          </button>
          {cur && (
            <button type="button" className="btn-primary" onClick={() => onAssignMacroToSelectedKey(selectedIdx)}>
              Assign #{selectedIdx} to Selected Key
            </button>
          )}
        </div>
      </div>

      {macros.length > 0 && (
        <div className="btn-group" style={{ marginBottom: 10 }}>
          {macros.map((m, idx) => (
            <button
              // biome-ignore lint/suspicious/noArrayIndexKey: key actions refer to macros by position, so the index is the macro's identity.
              key={idx}
              type="button"
              className={`btn-secondary ${selectedIdx === idx ? "active" : ""}`}
              onClick={() => setSelectedIdx(idx)}
            >
              #{idx}: {m.name}
            </button>
          ))}
        </div>
      )}

      {cur ? (
        <>
          <div className="toolbar-between">
            <label className="field" style={{ flex: 1 }}>
              <span>Macro Name</span>
              <input type="text" value={cur.name} onChange={(e) => updateCur({ name: e.target.value })} />
            </label>
            <div className="btn-group">
              <button
                type="button"
                className={`btn-secondary ${recording ? "active" : ""}`}
                disabled={Boolean(cur.words)}
                onClick={() => setRecording((r) => !r)}
              >
                {recording ? "Stop Recording" : "Record Keystrokes"}
              </button>
              <button
                type="button"
                className="btn-secondary"
                disabled={Boolean(cur.words)}
                onClick={() =>
                  updateCur({ steps: [...(cur.steps ?? []), { keyCode: "KC_A", delayMs: 15, isDown: true }] })
                }
              >
                + Step
              </button>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => {
                  setRecording(false);
                  onDeleteMacro(selectedIdx);
                  setSelectedIdx(Math.max(0, selectedIdx - 1));
                }}
              >
                Delete
              </button>
            </div>
          </div>

          {cur.words ? (
            <div className="muted" style={{ padding: "8px 0" }}>
              This macro came from the keyboard and can't be edited here.
            </div>
          ) : (
            <div className="macro-steps">
              {(cur.steps ?? []).map((st, sidx) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: steps have no identity of their own, and every field in a row is controlled, so a row reused for the next step after a deletion shows that step's values.
                <div key={sidx} className="macro-step-row">
                  <input
                    type="text"
                    value={st.keyCode}
                    aria-label={`Macro step ${sidx} key`}
                    onChange={(e) =>
                      updateCur({
                        steps: (cur.steps ?? []).map((x, i) => (i === sidx ? { ...x, keyCode: e.target.value } : x)),
                      })
                    }
                  />
                  <select
                    value={st.isDown ? "down" : "up"}
                    aria-label={`Macro step ${sidx} direction`}
                    onChange={(e) =>
                      updateCur({
                        steps: (cur.steps ?? []).map((x, i) =>
                          i === sidx ? { ...x, isDown: e.target.value === "down" } : x,
                        ),
                      })
                    }
                  >
                    <option value="down">Press</option>
                    <option value="up">Release</option>
                  </select>
                  <input
                    type="number"
                    min={1}
                    max={65000}
                    value={st.delayMs}
                    aria-label={`Macro step ${sidx} delay`}
                    onChange={(e) =>
                      updateCur({
                        steps: (cur.steps ?? []).map((x, i) =>
                          i === sidx ? { ...x, delayMs: Number(e.target.value) } : x,
                        ),
                      })
                    }
                  />
                  <span className="muted" style={{ fontSize: 11 }}>
                    ms
                  </span>
                  <button
                    type="button"
                    className="btn-secondary"
                    aria-label={`Remove macro step ${sidx}`}
                    onClick={() => updateCur({ steps: (cur.steps ?? []).filter((_, i) => i !== sidx) })}
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}
        </>
      ) : (
        <p className="muted">No macros yet. Click &ldquo;+ New Macro&rdquo; to record or build a keystroke sequence.</p>
      )}
    </div>
  );
};
