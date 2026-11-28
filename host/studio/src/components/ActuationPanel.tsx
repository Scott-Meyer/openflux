import type React from "react";
import { useState } from "react";
import type { ActuationMode, KeyglassConfig, PerKeyActuationOverride, PreviewKeyRect } from "../types/keyglass";
import { ANSI_ROW_COUNTS, formatKeyActionLabel, humanKeyName, ISO_ROW_COUNTS } from "../types/keyglass";
import {
  releaseAfterActuationMove,
  releasePointOf,
  releasePointProblem,
  TRAVEL_MAX_MM,
  validReleasePoint,
} from "./actuationTravel";
import { NameForm, useConfigEdits } from "./ConfigEditDialog";
import { addActuationMode, deleteActuationMode, maxActuationModes, renameActuationMode } from "./layerModeEdits";

interface ActuationPanelProps {
  config: KeyglassConfig;
  previewRects?: PreviewKeyRect[];
  selectedKeyIndex: number | null;
  keymapBlocked?: boolean;
  onSelectKey: (keyIdx: number) => void;
  onChangeActuation: (modes: ActuationMode[]) => void;
  /** Applies a mode edit, which can change the modes and the profiles that use them together. */
  onApplyConfigEdit?: (patch: Partial<KeyglassConfig>) => void;
}

export const ActuationPanel: React.FC<ActuationPanelProps> = ({
  config,
  previewRects,
  selectedKeyIndex,
  keymapBlocked = false,
  onSelectKey,
  onChangeActuation,
  onApplyConfigEdit,
}) => {
  const modes = config.actuationModes ?? [];
  const [activeModeIdx, setActiveModeIdx] = useState(0);
  const [renamingIdx, setRenamingIdx] = useState<number | null>(null);
  const modeEdits = useConfigEdits((patch) => onApplyConfigEdit?.(patch));
  // A release point the panel refused, with the reason, keyed by the slider it came from.
  const [refusedRelease, setRefusedRelease] = useState<{ slider: string; message: string } | null>(null);
  const layout = config.keymap.selectedFrameLayoutType;
  const rowCounts = layout === "iso" ? ISO_ROW_COUNTS : ANSI_ROW_COUNTS;
  const kmData = config.keymap.keymaps[layout];
  const baseLayer = kmData?.layers[0];
  const activeKey = selectedKeyIndex ?? 0;

  const updateModeGlobal = (idx: number, patch: Partial<ActuationMode>) => {
    if (keymapBlocked) return;
    const next = modes.map((m, i) =>
      i === idx
        ? { ...m, ...patch, firmwareDefault: undefined, touched: true, globalTouched: true }
        : { ...m, touched: m.touched === true },
    );
    onChangeActuation(next);
  };

  const updateKeyOverride = (idx: number, keyIdx: number, override: PerKeyActuationOverride | null) => {
    if (keymapBlocked) return;
    const next = modes.map((m, i) => {
      if (i !== idx) return { ...m, touched: m.touched === true };
      const existingScoped = m.perKeyOverrides ?? {};
      const layoutMap = { ...(existingScoped[layout] ?? {}) };
      if (override === null) {
        layoutMap[String(keyIdx)] = {
          actuationPointMm: m.actuationPointMm,
          rapidTrigger: m.rapidTrigger,
          rapidTriggerSensitivityMm: m.rapidTriggerSensitivityMm,
          releasePointMm: m.releasePointMm,
          resetToDefault: true,
          touched: true,
        };
      } else {
        layoutMap[String(keyIdx)] = { ...override, resetToDefault: false, touched: true };
      }
      const activeOverrideCount = Object.values(layoutMap).filter((v) => !v.resetToDefault).length;
      return {
        ...m,
        mixedPerKey: activeOverrideCount > 0,
        perKeyOverrides: {
          ...existingScoped,
          [layout]: layoutMap,
        },
        touched: true,
        globalTouched: m.globalTouched ?? false,
      };
    });
    onChangeActuation(next);
  };

  const curMode = modes[activeModeIdx] ?? modes[0];
  const curScopedMap = curMode?.perKeyOverrides?.[layout] ?? {};
  const rawKeyOv = curScopedMap[String(activeKey)];
  const curKeyOv = rawKeyOv && !rawKeyOv.resetToDefault ? rawKeyOv : undefined;
  const curActMm = curKeyOv?.actuationPointMm ?? curMode?.actuationPointMm ?? 1.2;
  const curRt = curKeyOv?.rapidTrigger ?? curMode?.rapidTrigger ?? false;
  const curSensMm = curKeyOv?.rapidTriggerSensitivityMm ?? curMode?.rapidTriggerSensitivityMm ?? 0.3;
  const curRelMm = releasePointOf(curKeyOv ?? curMode ?? { actuationPointMm: curActMm });
  const keySlider = `key:${activeModeIdx}:${layout}:${activeKey}`;

  /** Applies a release point from `slider`, or explains why it can't be one and leaves the setting as it is. */
  const changeRelease = (slider: string, actuationMm: number, releaseMm: number, apply: () => void) => {
    const problem = releasePointProblem(actuationMm, releaseMm);
    if (problem) {
      setRefusedRelease({ slider, message: problem });
      return;
    }
    setRefusedRelease(null);
    apply();
  };

  /** The refused change from `slider`, or the problem with the stored release point when there is one. */
  const releaseNotice = (slider: string, actuationMm: number, releaseMm: number) => {
    const message =
      refusedRelease?.slider === slider ? refusedRelease.message : releasePointProblem(actuationMm, releaseMm);
    return message ? (
      <p className="error-text" role="alert">
        {message}
      </p>
    ) : null;
  };

  let flatCursor = 0;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      {onApplyConfigEdit && (
        <fieldset className="toolbar-between" aria-label="Manage Actuation Modes" disabled={keymapBlocked}>
          <span className="muted">
            {modes.length} of {maxActuationModes(config)} actuation modes
          </span>
          <NameForm
            label="New mode name"
            submitLabel="Add Mode"
            disabled={keymapBlocked || modes.length >= maxActuationModes(config)}
            onSubmit={(name) => {
              const added = modes.length;
              modeEdits.request("Add Mode", addActuationMode(config, name), () => setActiveModeIdx(added));
            }}
          />
        </fieldset>
      )}
      {modeEdits.notice && <p className="status-msg">{modeEdits.notice}</p>}
      {modeEdits.dialog}
      <div className="panel-grid">
        {modes.map((mode, idx) => (
          // biome-ignore lint/a11y/useKeyWithClickEvents lint/a11y/noStaticElementInteractions: clicking anywhere on the card is a pointer shortcut; the title button selects the mode from the keyboard.
          <div
            className={`card ${activeModeIdx === idx ? "card-focused" : ""}`}
            key={mode.id}
            onClick={() => setActiveModeIdx(idx)}
          >
            <div className="toolbar-between">
              <h3>
                <button
                  type="button"
                  className="card-title-button"
                  aria-pressed={activeModeIdx === idx}
                  onClick={() => setActiveModeIdx(idx)}
                >
                  {mode.name}
                </button>
              </h3>
              <span className="pill">
                {mode.rapidTrigger ? "Rapid Trigger" : "Traditional"}
                {mode.firmwareDefault ? " · Keyboard Default" : ""}
                {mode.mixedPerKey ? " · Per-Key" : ""}
              </span>
            </div>
            {onApplyConfigEdit &&
              (renamingIdx === idx ? (
                <NameForm
                  label={`Rename ${mode.name}`}
                  submitLabel="Rename"
                  initial={mode.name}
                  disabled={keymapBlocked}
                  onSubmit={(name) =>
                    modeEdits.request("Rename Mode", renameActuationMode(config, idx, name), () => setRenamingIdx(null))
                  }
                  onCancel={() => setRenamingIdx(null)}
                />
              ) : (
                <div className="btn-group">
                  <button
                    type="button"
                    className="btn-secondary"
                    disabled={keymapBlocked}
                    onClick={() => setRenamingIdx(idx)}
                  >
                    Rename
                  </button>
                  <button
                    type="button"
                    className="btn-secondary"
                    disabled={keymapBlocked || modes.length >= maxActuationModes(config)}
                    onClick={() =>
                      modeEdits.request("Duplicate Mode", addActuationMode(config, `${mode.name} copy`, idx), () =>
                        setActiveModeIdx(modes.length),
                      )
                    }
                  >
                    Duplicate
                  </button>
                  <button
                    type="button"
                    className="btn-secondary"
                    disabled={keymapBlocked || modes.length <= 1}
                    aria-label={`Delete ${mode.name}`}
                    onClick={() =>
                      modeEdits.request(`Delete ${mode.name}`, deleteActuationMode(config, idx), () =>
                        setActiveModeIdx((cur) => (cur > idx ? cur - 1 : cur === idx ? 0 : cur)),
                      )
                    }
                  >
                    Delete
                  </button>
                </div>
              ))}
            <label className="field">
              <span>Actuation Point ({mode.actuationPointMm.toFixed(1)} mm)</span>
              <input
                type="range"
                min={0.1}
                max={TRAVEL_MAX_MM}
                step={0.1}
                value={mode.actuationPointMm}
                onChange={(e) => {
                  const next = Number(e.target.value);
                  setRefusedRelease(null);
                  updateModeGlobal(
                    idx,
                    mode.rapidTrigger
                      ? { actuationPointMm: next }
                      : {
                          actuationPointMm: next,
                          releasePointMm: releaseAfterActuationMove(mode.actuationPointMm, releasePointOf(mode), next),
                        },
                  );
                }}
              />
            </label>
            {!mode.rapidTrigger && (
              <>
                <label className="field">
                  <span>Release Point ({releasePointOf(mode).toFixed(1)} mm)</span>
                  <input
                    type="range"
                    min={0}
                    max={TRAVEL_MAX_MM}
                    step={0.1}
                    value={releasePointOf(mode)}
                    onChange={(e) => {
                      const next = Number(e.target.value);
                      changeRelease(`mode:${idx}`, mode.actuationPointMm, next, () =>
                        updateModeGlobal(idx, { releasePointMm: next }),
                      );
                    }}
                  />
                </label>
                <small className="muted">
                  A held key releases when it rises back to the release point, which sits above the actuation point. The
                  gap between them keeps a key resting near the actuation point from chattering.
                </small>
                {releaseNotice(`mode:${idx}`, mode.actuationPointMm, releasePointOf(mode))}
              </>
            )}
            <label className="checkbox-inline">
              <input
                type="checkbox"
                checked={mode.rapidTrigger}
                onChange={(e) => {
                  setRefusedRelease(null);
                  updateModeGlobal(idx, {
                    rapidTrigger: e.target.checked,
                    type: e.target.checked ? 2 : 1,
                    rapidTriggerSensitivityMm: mode.rapidTriggerSensitivityMm ?? 0.3,
                    releasePointMm: validReleasePoint(mode.actuationPointMm, releasePointOf(mode)),
                  });
                }}
              />
              Rapid Trigger
            </label>
            {mode.rapidTrigger && (
              <label className="field">
                <span>Rapid Trigger Reset Sensitivity ({(mode.rapidTriggerSensitivityMm ?? 0.3).toFixed(1)} mm)</span>
                <input
                  type="range"
                  min={0.1}
                  max={1.5}
                  step={0.1}
                  value={mode.rapidTriggerSensitivityMm ?? 0.3}
                  onChange={(e) => updateModeGlobal(idx, { rapidTriggerSensitivityMm: Number(e.target.value) })}
                />
              </label>
            )}
          </div>
        ))}
      </div>

      {curMode && (
        <div className="card">
          <div className="toolbar-between">
            <h3>
              Per-Key Actuation &amp; Rapid Trigger ({curMode.name} · {layout.toUpperCase()} ·{" "}
              {humanKeyName(config, activeKey)})
            </h3>
            <span className="pill">
              {Object.values(curScopedMap).filter((v) => !v.resetToDefault).length} per-key override(s)
            </span>
          </div>

          <div className="keymap-layout">
            {previewRects && previewRects.length === rowCounts.reduce((a, b) => a + b, 0) ? (
              <fieldset className="phys-matrix-canvas" aria-label="Per-Key Actuation Matrix">
                {(() => {
                  const minY = Math.min(...previewRects.map((r) => r.y));
                  const maxY = Math.max(...previewRects.map((r) => r.y + r.h));
                  const spanY = Math.max(100, maxY - minY + 24);
                  return previewRects.map((r) => {
                    const keyIdx = r.keyIndex;
                    const ka = baseLayer?.keyActions[keyIdx];
                    const rawOv = curScopedMap[String(keyIdx)];
                    const ov = rawOv && !rawOv.resetToDefault ? rawOv : undefined;
                    const lbl = formatKeyActionLabel(ka, keyIdx);
                    return (
                      <button
                        key={keyIdx}
                        type="button"
                        className={`phys-key-abs ${keyIdx === activeKey ? "selected" : ""} ${ov ? "overridden" : ""}`}
                        onClick={() => onSelectKey(keyIdx)}
                        style={{
                          left: `${(r.x / 1920) * 100}%`,
                          top: `${((r.y - minY + 12) / spanY) * 100}%`,
                          width: `${(r.w / 1920) * 100}%`,
                          height: `${(r.h / spanY) * 100}%`,
                          borderColor: ov ? "#f59e0b" : undefined,
                        }}
                      >
                        <span className="key-lbl">{lbl.slice(0, 6)}</span>
                        <span className="key-idx" style={{ color: ov ? "#fbbf24" : "#2dd4bf" }}>
                          {(ov ? ov.actuationPointMm : curMode.actuationPointMm).toFixed(1)}
                        </span>
                      </button>
                    );
                  });
                })()}
              </fieldset>
            ) : (
              <fieldset className="key-matrix" aria-label="Per-Key Actuation Matrix">
                {rowCounts.map((count, rowIdx) => {
                  const rowStart = flatCursor;
                  flatCursor += count;
                  return (
                    // biome-ignore lint/suspicious/noArrayIndexKey: the row layout is a fixed constant for the keyboard layout.
                    <div className="key-row" key={rowIdx}>
                      {Array.from({ length: count }, (_, colIdx) => {
                        const keyIdx = rowStart + colIdx;
                        const ka = baseLayer?.keyActions[keyIdx];
                        const rawOv = curScopedMap[String(keyIdx)];
                        const ov = rawOv && !rawOv.resetToDefault ? rawOv : undefined;
                        const lbl = formatKeyActionLabel(ka, keyIdx);
                        return (
                          <button
                            key={keyIdx}
                            type="button"
                            className={`key-cap ${keyIdx === activeKey ? "selected" : ""} ${ov ? "overridden" : ""}`}
                            onClick={() => onSelectKey(keyIdx)}
                            style={{
                              borderColor: ov ? "#f59e0b" : undefined,
                            }}
                          >
                            <span className="key-lbl">{lbl.slice(0, 6)}</span>
                            <span className="key-idx" style={{ color: ov ? "#fbbf24" : "#2dd4bf" }}>
                              {(ov ? ov.actuationPointMm : curMode.actuationPointMm).toFixed(1)}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  );
                })}
              </fieldset>
            )}

            <div>
              {/* Travel depth SVG visualizer */}
              <svg viewBox="0 0 280 96" className="travel-svg" role="img" aria-label="Key Travel Curve">
                <rect x="0" y="0" width="280" height="96" rx="8" fill="#090d16" />
                <text x="16" y="20" fill="#9eb0cf" fontSize="10">
                  0.0 mm (Top)
                </text>
                <text x="264" y="20" textAnchor="end" fill="#9eb0cf" fontSize="10">
                  {TRAVEL_MAX_MM.toFixed(1)} mm (Bottom)
                </text>
                <rect x="16" y="34" width="248" height="14" rx="7" fill="#1b2438" />
                <rect
                  x="16"
                  y="34"
                  width={Math.max(8, (curActMm / TRAVEL_MAX_MM) * 248)}
                  height="14"
                  rx="7"
                  fill={curRt ? "#f59e0b" : "#2dd4bf"}
                />
                {!curRt && (
                  <rect x={14 + (curRelMm / TRAVEL_MAX_MM) * 248} y="30" width="4" height="22" rx="2" fill="#a78bfa" />
                )}
                <circle cx={16 + (curActMm / TRAVEL_MAX_MM) * 248} cy="41" r="6" fill="#ffffff" />
                <text x="16" y="72" fill="#e2e8f0" fontSize="11">
                  Actuation: {curActMm.toFixed(1)} mm ·{" "}
                  {curRt ? `Rapid Trigger Δ ${curSensMm.toFixed(1)} mm` : `Release: ${curRelMm.toFixed(1)} mm`}
                </text>
              </svg>

              <label className="field" style={{ marginTop: 10 }}>
                <span>
                  {humanKeyName(config, activeKey)} Actuation ({curActMm.toFixed(1)} mm)
                </span>
                <input
                  type="range"
                  min={0.1}
                  max={TRAVEL_MAX_MM}
                  step={0.1}
                  value={curActMm}
                  onChange={(e) => {
                    const next = Number(e.target.value);
                    setRefusedRelease(null);
                    updateKeyOverride(activeModeIdx, activeKey, {
                      actuationPointMm: next,
                      rapidTrigger: curRt,
                      rapidTriggerSensitivityMm: curSensMm,
                      releasePointMm: curRt ? curRelMm : releaseAfterActuationMove(curActMm, curRelMm, next),
                    });
                  }}
                />
              </label>

              <label className="checkbox-inline">
                <input
                  type="checkbox"
                  checked={curRt}
                  onChange={(e) => {
                    setRefusedRelease(null);
                    updateKeyOverride(activeModeIdx, activeKey, {
                      actuationPointMm: curActMm,
                      rapidTrigger: e.target.checked,
                      rapidTriggerSensitivityMm: curSensMm,
                      releasePointMm: validReleasePoint(curActMm, curRelMm),
                    });
                  }}
                />
                Per-Key Rapid Trigger
              </label>

              {curRt && (
                <label className="field">
                  <span>
                    {humanKeyName(config, activeKey)} Reset Sensitivity ({curSensMm.toFixed(1)} mm)
                  </span>
                  <input
                    type="range"
                    min={0.1}
                    max={1.5}
                    step={0.1}
                    value={curSensMm}
                    onChange={(e) =>
                      updateKeyOverride(activeModeIdx, activeKey, {
                        actuationPointMm: curActMm,
                        rapidTrigger: true,
                        rapidTriggerSensitivityMm: Number(e.target.value),
                        releasePointMm: curRelMm,
                      })
                    }
                  />
                </label>
              )}

              {!curRt && (
                <>
                  <label className="field">
                    <span>
                      {humanKeyName(config, activeKey)} Release Point ({curRelMm.toFixed(1)} mm)
                    </span>
                    <input
                      type="range"
                      min={0}
                      max={TRAVEL_MAX_MM}
                      step={0.1}
                      value={curRelMm}
                      onChange={(e) => {
                        const next = Number(e.target.value);
                        changeRelease(keySlider, curActMm, next, () =>
                          updateKeyOverride(activeModeIdx, activeKey, {
                            actuationPointMm: curActMm,
                            rapidTrigger: false,
                            rapidTriggerSensitivityMm: curSensMm,
                            releasePointMm: next,
                          }),
                        );
                      }}
                    />
                  </label>
                  {releaseNotice(keySlider, curActMm, curRelMm)}
                </>
              )}

              {curKeyOv && (
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => updateKeyOverride(activeModeIdx, activeKey, null)}
                >
                  Reset {humanKeyName(config, activeKey)} to Mode Default
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
