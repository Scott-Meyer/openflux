import type React from "react";
import { useEffect, useState } from "react";
import type { KeyAction, MacroDefinition } from "../types/openflux";
import { canonicalKeycode, KEYCODE_CATEGORIES } from "./keycodes";

interface KeycodePickerModalProps {
  action: KeyAction;
  macros: MacroDefinition[];
  layerCount: number;
  onApply: (next: KeyAction) => void;
  onClose: () => void;
}

export function domCodeToQmk(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return `KC_${code.slice(3)}`;
  if (/^Digit[0-9]$/.test(code)) return `KC_${code.slice(5)}`;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return `KC_${code}`;
  if (/^Numpad[0-9]$/.test(code)) return `KC_KP_${code.slice(6)}`;
  const map: Record<string, string> = {
    Space: "KC_SPACE",
    Enter: "KC_ENTER",
    Escape: "KC_ESCAPE",
    Backspace: "KC_BACKSPACE",
    Tab: "KC_TAB",
    CapsLock: "KC_CAPS_LOCK",
    Delete: "KC_DELETE",
    Home: "KC_HOME",
    End: "KC_END",
    PageUp: "KC_PAGE_UP",
    PageDown: "KC_PAGE_DOWN",
    ArrowUp: "KC_UP",
    ArrowDown: "KC_DOWN",
    ArrowLeft: "KC_LEFT",
    ArrowRight: "KC_RIGHT",
    Minus: "KC_MINUS",
    Equal: "KC_EQUAL",
    BracketLeft: "KC_LEFT_BRACKET",
    BracketRight: "KC_RIGHT_BRACKET",
    Backslash: "KC_BACKSLASH",
    Semicolon: "KC_SEMICOLON",
    Quote: "KC_QUOTE",
    Backquote: "KC_GRAVE",
    Comma: "KC_COMMA",
    Period: "KC_DOT",
    Slash: "KC_SLASH",
    IntlBackslash: "KC_NONUS_BACKSLASH",
    Insert: "KC_INSERT",
    PrintScreen: "KC_PRINT_SCREEN",
    ScrollLock: "KC_SCROLL_LOCK",
    Pause: "KC_PAUSE",
    ContextMenu: "KC_APPLICATION",
    NumLock: "KC_NUM_LOCK",
    NumpadDivide: "KC_KP_SLASH",
    NumpadMultiply: "KC_KP_ASTERISK",
    NumpadSubtract: "KC_KP_MINUS",
    NumpadAdd: "KC_KP_PLUS",
    NumpadEnter: "KC_KP_ENTER",
    NumpadDecimal: "KC_KP_DOT",
    NumpadEqual: "KC_KP_EQUAL",
    NumpadComma: "KC_KP_COMMA",
    IntlRo: "KC_INTERNATIONAL_1",
    KanaMode: "KC_INTERNATIONAL_2",
    IntlYen: "KC_INTERNATIONAL_3",
    Convert: "KC_INTERNATIONAL_4",
    NonConvert: "KC_INTERNATIONAL_5",
    Lang1: "KC_LANGUAGE_1",
    Lang2: "KC_LANGUAGE_2",
  };
  return map[code] ?? null;
}

const MODIFIER_NAMES = [
  "Ctrl",
  "Shift",
  "Opt / Alt",
  "Cmd / Win",
  "Right Ctrl",
  "Right Shift",
  "Right Alt",
  "Right Cmd",
];

/** The modifiers set in a key action's modifier bits, by name. */
function modifierNames(mods: number): string[] {
  return MODIFIER_NAMES.filter((_, bit) => (mods & (1 << bit)) !== 0);
}

export const KeycodePickerModal: React.FC<KeycodePickerModalProps> = ({
  action,
  macros,
  layerCount,
  onApply,
  onClose,
}) => {
  const [mode, setMode] = useState<"key" | "layer" | "macro">(
    action.type === 2 ? "layer" : action.type === 1 ? "macro" : "key",
  );
  const [keyCode, setKeyCode] = useState(action.keyCode ?? "KC_A");
  const [mods, setMods] = useState(action.modifiers ?? 0);
  const [lct, setLct] = useState(action.layerControlType ?? "momentaryLayer");
  const [targetLayer, setTargetLayer] = useState(action.targetLayer ?? 1);
  const [macroId, setMacroId] = useState(action.macroId ?? 0);
  const [capturing, setCapturing] = useState(false);

  const toggleMod = (bit: number) => setMods((m) => m ^ bit);

  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const qmk = domCodeToQmk(e.code);
      if (qmk) {
        let nextMods = 0;
        if (e.ctrlKey) nextMods |= 0x01;
        if (e.shiftKey) nextMods |= 0x02;
        if (e.altKey) nextMods |= 0x04;
        if (e.metaKey) nextMods |= 0x08;
        setKeyCode(qmk);
        setMods(nextMods);
        setMode("key");
        setCapturing(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [capturing]);

  return (
    <div className="modal-backdrop" role="dialog" aria-label="Visual Keycode Picker">
      <div className="modal-card modal-wide">
        <div className="toolbar-between">
          <h3>Visual Keycode &amp; Binding Picker</h3>
          <div className="btn-group">
            {(["key", "layer", "macro"] as const).map((m) => (
              <button
                key={m}
                type="button"
                className={`btn-secondary ${mode === m ? "active" : ""}`}
                onClick={() => setMode(m)}
              >
                {m === "key" ? "Key + Modifiers" : m === "layer" ? "Fn Layer" : "Macro"}
              </button>
            ))}
            <button type="button" className="btn-secondary" onClick={onClose}>
              Close
            </button>
          </div>
        </div>

        {mode === "key" && (
          <>
            <div className="toolbar-between">
              <div className="checkbox-row">
                <label>
                  <input type="checkbox" checked={(mods & 0x01) !== 0} onChange={() => toggleMod(0x01)} /> Ctrl
                </label>
                <label>
                  <input type="checkbox" checked={(mods & 0x02) !== 0} onChange={() => toggleMod(0x02)} /> Shift
                </label>
                <label>
                  <input type="checkbox" checked={(mods & 0x04) !== 0} onChange={() => toggleMod(0x04)} /> Opt / Alt
                </label>
                <label>
                  <input type="checkbox" checked={(mods & 0x08) !== 0} onChange={() => toggleMod(0x08)} /> Cmd / Win
                </label>
              </div>
              <button
                type="button"
                className={`btn-secondary ${capturing ? "active" : ""}`}
                onClick={() => setCapturing((c) => !c)}
              >
                {capturing ? "Press any keystroke..." : "Capture Keystroke"}
              </button>
            </div>
            <div className="picker-sections">
              {KEYCODE_CATEGORIES.map((cat) => (
                <div key={cat.label} className="picker-cat">
                  <small className="muted">{cat.label}</small>
                  <div className="picker-keys">
                    {cat.keys.map((k) => (
                      <button
                        key={k.code}
                        type="button"
                        className={`key-cap-mini ${canonicalKeycode(keyCode) === k.code ? "selected" : ""}`}
                        onClick={() => setKeyCode(k.code)}
                        title={`${k.label} (${k.code})`}
                      >
                        {k.legend}
                      </button>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </>
        )}

        {mode === "layer" && (
          <div className="row-2" style={{ margin: "16px 0" }}>
            <label className="field">
              <span>Layer Activation Mode</span>
              <select value={lct} onChange={(e) => setLct(e.target.value)}>
                <option value="momentaryLayer">Momentary (Hold Fn)</option>
                <option value="toggleLayer">Toggle Layer</option>
                <option value="switchToLayer">Switch To Layer</option>
              </select>
            </label>
            <label className="field">
              <span>Target Layer</span>
              <input
                type="number"
                min={0}
                max={Math.max(1, layerCount - 1)}
                value={targetLayer}
                onChange={(e) => setTargetLayer(Number(e.target.value))}
              />
            </label>
          </div>
        )}

        {mode === "macro" && (
          <div style={{ margin: "16px 0" }}>
            <label className="field">
              <span>Macro Slot</span>
              <select value={macroId} onChange={(e) => setMacroId(Number(e.target.value))}>
                {macros.length === 0 ? (
                  <option value={0}>Macro #0 (create sequence in Macro Editor)</option>
                ) : (
                  macros.map((m, idx) => (
                    // biome-ignore lint/suspicious/noArrayIndexKey: key actions refer to macros by position, so the index is the macro's identity.
                    <option key={idx} value={idx}>
                      #{idx}: {m.name} (
                      {m.words ? `${m.words.length} recorded actions` : `${m.steps?.length ?? 0} steps`})
                    </option>
                  ))
                )}
              </select>
            </label>
          </div>
        )}

        <div className="toolbar-between" style={{ marginTop: 12 }}>
          <span className="pill">
            {mode === "key"
              ? [...modifierNames(mods), keyCode].join(" + ")
              : mode === "layer"
                ? `${lct} → Layer ${targetLayer}`
                : `MACRO_${macroId}`}
          </span>
          <button
            type="button"
            className="btn-primary"
            onClick={() => {
              if (mode === "key") {
                onApply({ type: 0, keyCode, modifiers: mods > 0 ? mods : null });
              } else if (mode === "layer") {
                onApply({ type: 2, layerControlType: lct, targetLayer });
              } else {
                onApply({ type: 1, macroId });
              }
              onClose();
            }}
          >
            Apply Binding
          </button>
        </div>
      </div>
    </div>
  );
};
