import type React from "react";
import { useEffect, useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import type {
  AppearanceTheme,
  AssetCatalog,
  KeyAction,
  KeymapConfig,
  MacroDefinition,
  OpenFluxConfig,
  PreviewKeyRect,
} from "../types/openflux";
import {
  ANSI_ROW_COUNTS,
  argbToHexColor,
  formatKeyActionLabel,
  hexColorToArgb,
  humanKeyName,
  ISO_ROW_COUNTS,
} from "../types/openflux";
import { IconBrowserModal } from "./AssetBrowsers";
import { NameForm, useConfigEdits } from "./ConfigEditDialog";
import { KeycodePickerModal } from "./KeycodePickerModal";
import { keycodeLegend } from "./keycodes";
import { addLayer, deleteLayer, layerCount, maxLayers, renameLayer } from "./layerModeEdits";
import { MacroEditorPanel } from "./MacroEditorPanel";

type MacrosField = KeymapConfig["macros"];

/** Whether the keymap keeps a macro list per layout (`{ ansi: [...], iso: [...] }`) rather than one shared list. */
function isLayoutScoped(field: MacrosField): boolean {
  return Boolean(
    field &&
      !Array.isArray(field) &&
      typeof field === "object" &&
      (Array.isArray((field as Record<string, unknown>).ansi) || Array.isArray((field as Record<string, unknown>).iso)),
  );
}

/** The macros a layout uses: the shared list, or the layout's own list. */
function macrosForLayout(field: MacrosField, layout: string): MacroDefinition[] {
  if (Array.isArray(field)) return field;
  const scoped = (field as Record<string, unknown> | undefined)?.[layout];
  return Array.isArray(scoped) ? (scoped as MacroDefinition[]) : [];
}

/** The macros field with `layout`'s macros replaced by `next`, keeping its shared or per-layout shape. */
function withMacrosForLayout(field: MacrosField, layout: string, next: MacroDefinition[]): MacrosField {
  return isLayoutScoped(field) ? ({ ...(field as Record<string, unknown>), [layout]: next } as MacrosField) : next;
}

interface KeymapStylemapEditorProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  previewRects?: PreviewKeyRect[];
  config: OpenFluxConfig;
  activeLayer: number;
  selectedKeyIndex: number | null;
  keymapBlocked?: boolean;
  onSelectLayer: (layer: number) => void;
  onSelectKey: (keyIdx: number) => void;
  onChangeAppearance: (theme: AppearanceTheme) => void;
  onChangeKeymap: (keymap: KeymapConfig) => void;
  /** Applies a layer edit, which can change the keymap, profiles and themes together. */
  onApplyConfigEdit?: (patch: Partial<OpenFluxConfig>) => void;
}

export const KeymapStylemapEditor: React.FC<KeymapStylemapEditorProps> = ({
  client,
  catalog,
  previewRects,
  config,
  activeLayer,
  selectedKeyIndex,
  keymapBlocked = false,
  onSelectLayer,
  onSelectKey,
  onChangeAppearance,
  onChangeKeymap,
  onApplyConfigEdit,
}) => {
  const layerEdits = useConfigEdits((patch) => onApplyConfigEdit?.(patch));
  const layout = config.keymap.selectedFrameLayoutType;
  const rowCounts = layout === "iso" ? ISO_ROW_COUNTS : ANSI_ROW_COUNTS;
  const kmData = config.keymap.keymaps[layout] ?? {
    defaultLayer: 0,
    layers: [{ name: "Base", keyActions: [] }],
  };
  const layer = kmData.layers[activeLayer] ?? kmData.layers[0];
  const theme =
    config.appearance.selectedId !== "default"
      ? (config.appearance.customThemes.find((t) => t.id === config.appearance.selectedId) ??
        config.appearance.defaultTheme)
      : config.appearance.defaultTheme;
  const stylemaps = theme.stylemaps ?? {};
  const styleLayer = stylemaps[String(activeLayer)] ?? { styles: {} };
  const stylesMap = styleLayer.styles ?? {};
  const macros = macrosForLayout(config.keymap.macros, layout);

  const activeKey = selectedKeyIndex ?? 0;
  const activeAction: KeyAction = layer?.keyActions[activeKey] ?? { keyCode: "KC_A", type: 0 };
  const activeStyle = stylesMap[String(activeKey)] ?? {};

  const formatActionDraft = (act: KeyAction): string => {
    if (act.type === 2) {
      const kind =
        act.layerControlType === "toggleLayer"
          ? "TOGGLE"
          : act.layerControlType === "switchToLayer"
            ? "SWITCH"
            : "MOMENTARY";
      return `FN_${kind}_L${act.targetLayer ?? 1}`;
    }
    if (act.type === 1) {
      return `MACRO_${act.macroId ?? 0}`;
    }
    return act.keyCode ?? "";
  };

  const initialDraft = formatActionDraft(activeAction);
  const [keyCodeDraft, setKeyCodeDraft] = useState<string>(initialDraft);
  const [inspectorTab, setInspectorTab] = useState<"keys" | "media" | "layers" | "style">("keys");
  const [showKeyPicker, setShowKeyPicker] = useState(false);
  const [showIconPicker, setShowIconPicker] = useState(false);

  // biome-ignore lint/correctness/useExhaustiveDependencies: selecting another key, layer or layout discards an unsubmitted draft even when the new key's action formats the same.
  useEffect(() => {
    setKeyCodeDraft(initialDraft);
  }, [layout, activeLayer, activeKey, initialDraft]);

  const updateKeyStyle = (patch: {
    label?: string;
    iconAssetPath?: string;
    legendColor?: number;
    ringGlowColor?: number;
  }) => {
    const nextStylemaps = {
      ...stylemaps,
      [String(activeLayer)]: {
        ...styleLayer,
        styles: {
          ...stylesMap,
          [String(activeKey)]: {
            ...activeStyle,
            ...patch,
          },
        },
      },
    };
    onChangeAppearance({ ...theme, stylemaps: nextStylemaps });
  };

  const applyKeyAction = (nextAct: KeyAction) => {
    if (keymapBlocked) return;
    const nextLayers = kmData.layers.map((l, idx) => {
      if (idx !== activeLayer) return l;
      const nextActions = [...l.keyActions];
      while (nextActions.length <= activeKey) {
        nextActions.push({ keyCode: "KC_TRANSPARENT", type: 0 });
      }
      nextActions[activeKey] = nextAct;
      return { ...l, keyActions: nextActions };
    });
    onChangeKeymap({
      ...config.keymap,
      keymaps: {
        ...config.keymap.keymaps,
        [layout]: { ...kmData, layers: nextLayers },
      },
    });
  };

  const commitKeyCode = (rawCode: string) => {
    const code = rawCode.trim();
    if (!code || code === initialDraft.trim()) return;
    const fnMatch = code.match(/^FN_(?:(MOMENTARY|TOGGLE|SWITCH)_)?L(\d+)$/i);
    if (fnMatch) {
      const kind = (fnMatch[1] ?? "").toUpperCase();
      const lct =
        kind === "TOGGLE"
          ? "toggleLayer"
          : kind === "SWITCH"
            ? "switchToLayer"
            : kind === "MOMENTARY"
              ? "momentaryLayer"
              : (activeAction.layerControlType ?? "momentaryLayer");
      applyKeyAction({
        type: 2,
        layerControlType: lct,
        targetLayer: Number(fnMatch[2]),
      });
    } else if (/^MACRO_\d+$/i.test(code)) {
      applyKeyAction({ type: 1, macroId: Number(code.slice(6)) });
    } else {
      applyKeyAction({
        type: 0,
        keyCode: code,
        modifiers: activeAction.type === 0 ? (activeAction.modifiers ?? null) : null,
      });
    }
  };

  let flatCursor = 0;
  return (
    <div>
      <div className="keymap-layout">
        <div className="card">
          <div className="toolbar-between">
            <h3>
              Physical Matrix ({layout.toUpperCase()} · {rowCounts.reduce((a, b) => a + b, 0)} keys)
            </h3>
            <div className="btn-group">
              {(["ansi", "iso"] as const).map((l) => (
                <button
                  key={l}
                  type="button"
                  disabled={keymapBlocked}
                  className={`btn-secondary ${layout === l ? "active" : ""}`}
                  onClick={() => onChangeKeymap({ ...config.keymap, selectedFrameLayoutType: l })}
                >
                  {l.toUpperCase()}
                </button>
              ))}
              {kmData.layers.map((lay, idx) => (
                <button
                  // biome-ignore lint/suspicious/noArrayIndexKey: firmware layers are addressed by position, so the index is the layer's identity.
                  key={lay.name + idx}
                  type="button"
                  className={`btn-secondary ${activeLayer === idx ? "active" : ""}`}
                  onClick={() => onSelectLayer(idx)}
                >
                  L{idx}: {lay.name}
                </button>
              ))}
            </div>
          </div>
          {onApplyConfigEdit && (
            <fieldset className="toolbar-between layer-manage" aria-label="Manage Layers" disabled={keymapBlocked}>
              <NameForm
                label="New layer name"
                submitLabel="Add Layer"
                disabled={keymapBlocked || layerCount(config.keymap) >= maxLayers(config)}
                onSubmit={(name) => {
                  const added = layerCount(config.keymap);
                  layerEdits.request("Add Layer", addLayer(config, name), () => onSelectLayer(added));
                }}
              />
              <NameForm
                key={`rename-${activeLayer}-${layer?.name ?? ""}`}
                label={`Rename L${activeLayer}`}
                submitLabel="Rename"
                initial={layer?.name ?? ""}
                disabled={keymapBlocked}
                onSubmit={(name) => layerEdits.request("Rename Layer", renameLayer(config, activeLayer, name))}
              />
              <button
                type="button"
                className="btn-secondary"
                disabled={keymapBlocked || activeLayer === 0}
                title={activeLayer === 0 ? "The base layer can't be deleted" : undefined}
                onClick={() =>
                  layerEdits.request(
                    `Delete Layer ${layer?.name ?? activeLayer}`,
                    deleteLayer(config, activeLayer),
                    () => onSelectLayer(Math.max(0, activeLayer - 1)),
                  )
                }
              >
                Delete L{activeLayer}
              </button>
            </fieldset>
          )}
          {layerEdits.notice && <p className="status-msg">{layerEdits.notice}</p>}
          {layerEdits.dialog}

          {previewRects && previewRects.length === rowCounts.reduce((a, b) => a + b, 0) ? (
            <fieldset className="phys-matrix-canvas" aria-label="Physical Key Matrix">
              {(() => {
                const minY = Math.min(...previewRects.map((r) => r.y));
                const maxY = Math.max(...previewRects.map((r) => r.y + r.h));
                const spanY = Math.max(100, maxY - minY + 24);
                return previewRects.map((r) => {
                  const keyIdx = r.keyIndex;
                  const ka = layer?.keyActions[keyIdx];
                  const ov = stylesMap[String(keyIdx)];
                  const label = ov?.label || formatKeyActionLabel(ka, keyIdx);
                  const selected = keyIdx === activeKey;
                  return (
                    <button
                      key={keyIdx}
                      data-key-index={keyIdx}
                      type="button"
                      className={`phys-key-abs ${selected ? "selected" : ""}`}
                      onClick={() => onSelectKey(keyIdx)}
                      style={{
                        left: `${(r.x / 1920) * 100}%`,
                        top: `${((r.y - minY + 12) / spanY) * 100}%`,
                        width: `${(r.w / 1920) * 100}%`,
                        height: `${(r.h / spanY) * 100}%`,
                        borderColor: ov?.ringGlowColor ? argbToHexColor(ov.ringGlowColor) : undefined,
                        color: ov?.legendColor ? argbToHexColor(ov.legendColor) : undefined,
                      }}
                    >
                      <span className="key-lbl">{label.slice(0, 7)}</span>
                    </button>
                  );
                });
              })()}
            </fieldset>
          ) : (
            <fieldset className="key-matrix" aria-label="Physical Key Matrix">
              {rowCounts.map((count, rowIdx) => {
                const rowStart = flatCursor;
                flatCursor += count;
                return (
                  // biome-ignore lint/suspicious/noArrayIndexKey: the row layout is a fixed constant for the keyboard layout.
                  <div className="key-row" key={rowIdx}>
                    {Array.from({ length: count }, (_, colIdx) => {
                      const keyIdx = rowStart + colIdx;
                      const ka = layer?.keyActions[keyIdx];
                      const ov = stylesMap[String(keyIdx)];
                      const label =
                        ov?.label ||
                        (ka?.type === 1
                          ? `M${ka.macroId ?? 0}`
                          : ka?.type === 2
                            ? `FN${ka.targetLayer ?? 1}`
                            : ka?.keyCode && keycodeLegend(ka.keyCode)) ||
                        `#${keyIdx}`;
                      const selected = keyIdx === activeKey;
                      return (
                        <button
                          key={keyIdx}
                          data-key-index={keyIdx}
                          type="button"
                          className={`key-cap ${selected ? "selected" : ""}`}
                          onClick={() => onSelectKey(keyIdx)}
                          style={{
                            borderColor: ov?.ringGlowColor ? argbToHexColor(ov.ringGlowColor) : undefined,
                            color: ov?.legendColor ? argbToHexColor(ov.legendColor) : undefined,
                          }}
                        >
                          <span className="key-lbl">{label.slice(0, 6)}</span>
                        </button>
                      );
                    })}
                  </div>
                );
              })}
            </fieldset>
          )}
        </div>

        <div className="card">
          <div className="toolbar-between" style={{ marginBottom: 8 }}>
            <div>
              <h3 style={{ margin: 0 }}>
                {humanKeyName(config, activeKey)} · Layer {activeLayer}
              </h3>
              <small className="muted">
                Current action: <strong>{formatKeyActionLabel(activeAction, activeKey)}</strong>
              </small>
            </div>
            <button type="button" className="btn-secondary" onClick={() => setShowKeyPicker(true)}>
              All Keycodes...
            </button>
          </div>

          <div className="seg-pair" style={{ marginBottom: 12 }} role="tablist" aria-label="Key Inspector Sections">
            {(
              [
                ["keys", "Keys"],
                ["media", "Media & Display"],
                ["layers", "Fn / Layers"],
                ["style", "Appearance"],
              ] as const
            ).map(([tid, tlbl]) => (
              <button
                key={tid}
                type="button"
                className={`seg-btn ${inspectorTab === tid ? "active" : ""}`}
                onClick={() => setInspectorTab(tid)}
              >
                {tlbl}
              </button>
            ))}
          </div>

          {inspectorTab === "keys" && (
            <>
              {activeAction.type === 0 && (
                <div className="checkbox-row" style={{ marginBottom: 10 }}>
                  {[
                    { label: "Ctrl", bit: 0x01 },
                    { label: "Shift", bit: 0x02 },
                    { label: "Opt", bit: 0x04 },
                    { label: "Cmd", bit: 0x08 },
                  ].map((m) => (
                    <label key={m.label}>
                      <input
                        type="checkbox"
                        checked={((activeAction.modifiers ?? 0) & m.bit) !== 0}
                        onChange={() => {
                          const nextMods = (activeAction.modifiers ?? 0) ^ m.bit;
                          applyKeyAction({
                            type: 0,
                            keyCode: activeAction.keyCode ?? "KC_A",
                            modifiers: nextMods > 0 ? nextMods : null,
                          });
                        }}
                      />
                      {m.label}
                    </label>
                  ))}
                </div>
              )}
              <div style={{ display: "flex", flexWrap: "wrap", gap: 5, marginBottom: 12 }}>
                {[
                  ["Esc", "KC_ESCAPE"],
                  ["Tab", "KC_TAB"],
                  ["Space", "KC_SPACE"],
                  ["Enter", "KC_ENTER"],
                  ["Bksp", "KC_BACKSPACE"],
                  ["Del", "KC_DELETE"],
                  ["LCtrl", "KC_LCTL"],
                  ["LShift", "KC_LSHIFT"],
                  ["LOpt", "KC_LALT"],
                  ["LCmd", "KC_LGUI"],
                  ["RCmd", "KC_RGUI"],
                  ["↑", "KC_UP"],
                  ["↓", "KC_DOWN"],
                  ["←", "KC_LEFT"],
                  ["→", "KC_RIGHT"],
                  ["None", "KC_NO"],
                  ["Pass-Thru", "KC_TRANSPARENT"],
                ].map(([lbl, code]) => (
                  <button
                    key={code}
                    type="button"
                    className={`btn-secondary ${activeAction.type === 0 && activeAction.keyCode === code ? "active" : ""}`}
                    style={{ padding: "4px 8px", fontSize: 11 }}
                    disabled={keymapBlocked}
                    onClick={() => applyKeyAction({ type: 0, keyCode: code, modifiers: null })}
                  >
                    {lbl}
                  </button>
                ))}
              </div>
              <details style={{ marginBottom: 10 }}>
                <summary className="muted" style={{ cursor: "pointer", fontSize: 11 }}>
                  Advanced Keycode String
                </summary>
                <label className="field" style={{ marginTop: 6 }}>
                  <span>Key Action Code</span>
                  <input
                    type="text"
                    aria-label="Key Action Code"
                    disabled={keymapBlocked}
                    value={keyCodeDraft}
                    onChange={(e) => setKeyCodeDraft(e.target.value)}
                    onBlur={() => commitKeyCode(keyCodeDraft)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        commitKeyCode(keyCodeDraft);
                      }
                    }}
                  />
                </label>
              </details>
            </>
          )}

          {inspectorTab === "media" && (
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 12 }}>
              {[
                ["Play / Pause", "KC_MEDIA_PLAY_PAUSE"],
                ["Next Track", "KC_MEDIA_NEXT_TRACK"],
                ["Prev Track", "KC_MEDIA_PREV_TRACK"],
                ["Volume +", "KC_AUDIO_VOL_UP"],
                ["Volume -", "KC_AUDIO_VOL_DOWN"],
                ["Mute", "KC_AUDIO_MUTE"],
                ["Pomodoro Start/Pause", "KC_OF_START_PAUSE_POMODORO"],
                ["Pomodoro Reset", "KC_OF_RESET_POMODORO"],
                ["Next Profile", "KC_OF_PROFILE_CYCLE_NEXT"],
                ["Next Theme", "KC_OF_THEME_CYCLE_NEXT"],
                ["Next Actuation", "KC_OF_ACTUATION_MODE_NEXT"],
              ].map(([lbl, code]) => (
                <button
                  key={code}
                  type="button"
                  className={`btn-secondary ${activeAction.type === 0 && activeAction.keyCode === code ? "active" : ""}`}
                  style={{ padding: "5px 9px", fontSize: 11 }}
                  disabled={keymapBlocked}
                  onClick={() => applyKeyAction({ type: 0, keyCode: code, modifiers: null })}
                >
                  {lbl}
                </button>
              ))}
            </div>
          )}

          {inspectorTab === "layers" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 12 }}>
              <span className="muted" style={{ fontSize: 11 }}>
                Hold (Momentary) activates a layer while held; Toggle latches it on/off.
              </span>
              {kmData.layers.map((l, idx) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: firmware layers are addressed by position, so the index is the layer's identity.
                <div key={`${l.name}-${idx}`} className="toolbar-between" style={{ gap: 6 }}>
                  <span style={{ fontSize: 12 }}>
                    <strong>L{idx}</strong>: {l.name}
                  </span>
                  <div className="btn-group">
                    <button
                      type="button"
                      className="btn-secondary"
                      style={{ padding: "4px 8px", fontSize: 11 }}
                      disabled={keymapBlocked}
                      onClick={() => applyKeyAction({ type: 2, layerControlType: "momentaryLayer", targetLayer: idx })}
                    >
                      Hold (MO)
                    </button>
                    <button
                      type="button"
                      className="btn-secondary"
                      style={{ padding: "4px 8px", fontSize: 11 }}
                      disabled={keymapBlocked}
                      onClick={() => applyKeyAction({ type: 2, layerControlType: "toggleLayer", targetLayer: idx })}
                    >
                      Toggle (TG)
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}

          <div className="rail-divider" style={{ margin: "10px 0" }} />
          <label className="field">
            <span>Custom Legend Override</span>
            <input
              type="text"
              value={activeStyle.label ?? ""}
              placeholder="Uses the key's own label"
              onChange={(e) => updateKeyStyle({ label: e.target.value })}
            />
          </label>
          <div className="toolbar-between" style={{ marginBottom: 10 }}>
            <span style={{ fontSize: 12 }}>
              Key Icon: <strong>{activeStyle.iconAssetPath || "None"}</strong>
            </span>
            <div className="btn-group">
              {activeStyle.iconAssetPath && (
                <button type="button" className="btn-secondary" onClick={() => updateKeyStyle({ iconAssetPath: "" })}>
                  Clear
                </button>
              )}
              <button type="button" className="btn-secondary" onClick={() => setShowIconPicker(true)}>
                Pick Icon...
              </button>
            </div>
          </div>
          <div className="row-2">
            <label className="field">
              <span>Per-Key Legend Color</span>
              <input
                type="color"
                value={argbToHexColor(activeStyle.legendColor ?? theme.legendColor)}
                onChange={(e) => updateKeyStyle({ legendColor: hexColorToArgb(e.target.value) })}
              />
            </label>
            <label className="field">
              <span>Per-Key Ring Glow</span>
              <input
                type="color"
                value={argbToHexColor(activeStyle.ringGlowColor ?? theme.ringGlowColor)}
                onChange={(e) => updateKeyStyle({ ringGlowColor: hexColorToArgb(e.target.value) })}
              />
            </label>
          </div>
        </div>
      </div>

      <MacroEditorPanel
        macros={macros}
        onChangeMacros={(nextMacros) => {
          if (keymapBlocked) return;
          onChangeKeymap({ ...config.keymap, macros: withMacrosForLayout(config.keymap.macros, layout, nextMacros) });
        }}
        onDeleteMacro={(deletedIdx) => {
          if (keymapBlocked) return;
          const nextMacros = macros.filter((_, i) => i !== deletedIdx);
          const isScoped = isLayoutScoped(config.keymap.macros);
          const nextKeymaps: KeymapConfig["keymaps"] = {};
          for (const [lname, ldata] of Object.entries(config.keymap.keymaps)) {
            if (isScoped && lname !== layout) {
              nextKeymaps[lname] = ldata;
              continue;
            }
            nextKeymaps[lname] = {
              ...ldata,
              layers: ldata.layers.map((lay, layIdx) => ({
                ...lay,
                keyActions: lay.keyActions.map((ka) => {
                  if (ka.type !== 1) return ka;
                  const mid = ka.macroId ?? 0;
                  if (mid === deletedIdx) {
                    return { type: 0, keyCode: layIdx === 0 ? "KC_NO" : "KC_TRANSPARENT" };
                  }
                  if (mid > deletedIdx) {
                    return { ...ka, macroId: mid - 1 };
                  }
                  return ka;
                }),
              })),
            };
          }
          onChangeKeymap({
            ...config.keymap,
            keymaps: nextKeymaps,
            macros: withMacrosForLayout(config.keymap.macros, layout, nextMacros),
          });
        }}
        onAssignMacroToSelectedKey={(macroId) => applyKeyAction({ type: 1, macroId })}
      />

      {showKeyPicker && (
        <KeycodePickerModal
          action={activeAction}
          macros={macros}
          layerCount={kmData.layers.length}
          onApply={applyKeyAction}
          onClose={() => setShowKeyPicker(false)}
        />
      )}
      {showIconPicker && (
        <IconBrowserModal
          client={client}
          catalog={catalog}
          currentIcon={activeStyle.iconAssetPath ?? ""}
          onSelect={(iconId) => updateKeyStyle({ iconAssetPath: iconId })}
          onClose={() => setShowIconPicker(false)}
        />
      )}
    </div>
  );
};
