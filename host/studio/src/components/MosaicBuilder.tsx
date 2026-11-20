import type React from "react";
import { useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import type { MosaicConfig } from "../types/openflux";
import { ALL_MOSAIC_MODULES } from "../types/openflux";

interface MosaicBuilderProps {
  client?: DaemonClient;
  mosaic: MosaicConfig;
  onChangeMosaic: (next: MosaicConfig) => void;
  previewNode?: React.ReactNode;
}

const CLOCK_STYLE_OPTIONS: { typeId: number; label: string; desc: string }[] = [
  { typeId: 12, label: "Minimal", desc: "Clean floating typography clock" },
  { typeId: 2, label: "Digital", desc: "Bold numeric hours & seconds" },
  { typeId: 10, label: "Analog", desc: "Classic circular clock face" },
  { typeId: 11, label: "Flip", desc: "Split-flap retro digits" },
];

const CLOCK_TYPE_IDS = new Set([2, 10, 11, 12]);

/** Drag data type of a module dragged from the library onto a slot. */
const MODULE_DRAG_TYPE = "application/x-openflux-module";

/** The module a drop carries, or null when the drop didn't come from the library. */
const droppedModule = (data: DataTransfer, dragging: number | null): number | null => {
  const raw = data.getData(MODULE_DRAG_TYPE);
  const typeId = raw ? Number(raw) : dragging;
  return typeId !== null && ALL_MOSAIC_MODULES.some((m) => m.typeId === typeId) ? typeId : null;
};

export const MosaicBuilder: React.FC<MosaicBuilderProps> = ({ client, mosaic, onChangeMosaic, previewNode }) => {
  const [selectedSlot, setSelectedSlot] = useState(0);
  const [dragTypeId, setDragTypeId] = useState<number | null>(null);
  const [pomoStatus, setPomoStatus] = useState<string | null>(null);

  const slots = [0, 1, 2, 3].map((i) => mosaic.tileTypes[i] ?? 0);
  const configs = [0, 1, 2, 3].map((i) => mosaic.tileConfigs[i] ?? {});

  const assignSlot = (slotIdx: number, typeId: number) => {
    const nextTypes = [...slots];
    nextTypes[slotIdx] = typeId;
    onChangeMosaic({ tileTypes: nextTypes, tileConfigs: configs });
    setSelectedSlot(slotIdx);
  };

  const patchSlotConfig = (slotIdx: number, patch: Record<string, unknown>) => {
    const nextCfgs = configs.map((c, i) => (i === slotIdx ? { ...c, ...patch } : c));
    onChangeMosaic({ tileTypes: slots, tileConfigs: nextCfgs });
  };

  const activeType = slots[selectedSlot];
  const activeCfg = configs[selectedSlot];
  const isClockActive = CLOCK_TYPE_IDS.has(activeType);

  const consolidatedLibrary = ALL_MOSAIC_MODULES.filter((m) => !CLOCK_TYPE_IDS.has(m.typeId) || m.typeId === 12).map(
    (m) =>
      m.typeId === 12
        ? {
            ...m,
            name: "Clock",
            description: "Time & date with Digital, Analog, Flip, or Minimal style",
          }
        : m,
  );

  return (
    <div className="modules-studio">
      <div className="modules-main">
        {previewNode && <div className="studio-hero-preview">{previewNode}</div>}

        <ul className="mosaic-dock" aria-label="Mosaic Dock Slots">
          {slots.map((typeId, slotIdx) => {
            const isClk = CLOCK_TYPE_IDS.has(typeId);
            const clockMeta = CLOCK_STYLE_OPTIONS.find((c) => c.typeId === typeId);
            const meta = ALL_MOSAIC_MODULES.find((m) => m.typeId === typeId) ?? ALL_MOSAIC_MODULES[0];
            return (
              // biome-ignore lint/suspicious/noArrayIndexKey: the dock has a fixed set of slots, and the index is the slot's identity.
              <li key={slotIdx}>
                <button
                  type="button"
                  aria-label={`Mosaic Slot ${slotIdx + 1}`}
                  aria-pressed={selectedSlot === slotIdx}
                  className={`mosaic-slot ${selectedSlot === slotIdx ? "selected" : ""}`}
                  onClick={() => setSelectedSlot(slotIdx)}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={(e) => {
                    e.preventDefault();
                    const dropped = droppedModule(e.dataTransfer, dragTypeId);
                    setDragTypeId(null);
                    if (dropped !== null) assignSlot(slotIdx, dropped);
                  }}
                >
                  <span className="pill">Slot {slotIdx + 1}</span>
                  <strong>{isClk ? "Clock" : meta.name}</strong>
                  <small className="muted">{isClk && clockMeta ? clockMeta.label : meta.category}</small>
                </button>
              </li>
            );
          })}
        </ul>

        <div className="slot-inspector">
          <h4>Configure Slot {selectedSlot + 1}</h4>
          {isClockActive && (
            <div className="field" style={{ marginBottom: 8 }}>
              <span>Clock Style</span>
              <div className="seg-pair" style={{ maxWidth: 420, gridTemplateColumns: "repeat(4, 1fr)" }}>
                {CLOCK_STYLE_OPTIONS.map((opt) => (
                  <button
                    key={opt.typeId}
                    type="button"
                    className={`seg-btn ${activeType === opt.typeId ? "active" : ""}`}
                    onClick={() => assignSlot(selectedSlot, opt.typeId)}
                  >
                    {opt.label}
                  </button>
                ))}
              </div>
            </div>
          )}
          {activeType === 15 && (
            <div className="row-2" style={{ alignItems: "end" }}>
              <label className="field">
                <span>Weather City</span>
                <input
                  type="text"
                  value={String(activeCfg.city ?? "")}
                  onChange={(e) => patchSlotConfig(selectedSlot, { city: e.target.value })}
                />
              </label>
              {/* The config key is spelled `celcius` in stored configs; the keyboard also reads `celsius`. */}
              <div className="field">
                <span>Temperature Unit</span>
                <div className="seg-pair">
                  <button
                    type="button"
                    className={`seg-btn ${activeCfg.celcius ? "active" : ""}`}
                    onClick={() => patchSlotConfig(selectedSlot, { celcius: true })}
                  >
                    Celsius (°C)
                  </button>
                  <button
                    type="button"
                    className={`seg-btn ${!activeCfg.celcius ? "active" : ""}`}
                    onClick={() => patchSlotConfig(selectedSlot, { celcius: false })}
                  >
                    Fahrenheit (°F)
                  </button>
                </div>
              </div>
            </div>
          )}
          {activeType === 13 && (
            <div>
              <div className="row-3">
                <label className="field">
                  <span>Focus (min)</span>
                  <input
                    type="number"
                    value={String(activeCfg.main ?? "30")}
                    onChange={(e) => patchSlotConfig(selectedSlot, { main: e.target.value })}
                  />
                </label>
                <label className="field">
                  <span>Short Break</span>
                  <input
                    type="number"
                    value={String(activeCfg.shortBreak ?? "5")}
                    onChange={(e) => patchSlotConfig(selectedSlot, { shortBreak: e.target.value })}
                  />
                </label>
                <label className="field">
                  <span>Long Break</span>
                  <input
                    type="number"
                    value={String(activeCfg.longBreak ?? "15")}
                    onChange={(e) => patchSlotConfig(selectedSlot, { longBreak: e.target.value })}
                  />
                </label>
              </div>
              {client && (
                <div className="btn-group" style={{ marginTop: 10, alignItems: "center" }}>
                  <button
                    type="button"
                    className="btn-primary"
                    onClick={async () => {
                      try {
                        const r = await client.pomodoroAction("toggle");
                        const p = r.pomodoro;
                        setPomoStatus(
                          p
                            ? `${p.running ? "Running" : "Paused"} · ${Math.floor(p.remainingS / 60)}:${String(p.remainingS % 60).padStart(2, "0")}`
                            : r.message,
                        );
                      } catch (e) {
                        setPomoStatus(String(e));
                      }
                    }}
                  >
                    Start / Pause Timer
                  </button>
                  <button
                    type="button"
                    className="btn-secondary"
                    onClick={async () => {
                      try {
                        const r = await client.pomodoroAction("reset");
                        const p = r.pomodoro;
                        setPomoStatus(
                          p
                            ? `Reset · ${Math.floor(p.remainingS / 60)}:${String(p.remainingS % 60).padStart(2, "0")}`
                            : r.message,
                        );
                      } catch (e) {
                        setPomoStatus(String(e));
                      }
                    }}
                  >
                    Reset
                  </button>
                  {pomoStatus && (
                    <span className="muted" style={{ fontSize: 12 }}>
                      {pomoStatus}
                    </span>
                  )}
                </div>
              )}
            </div>
          )}
          {activeType === 14 && (
            <div className="row-2">
              <label className="field">
                <span>World Clock City</span>
                <input
                  type="text"
                  value={String(activeCfg.city ?? "Tokyo")}
                  onChange={(e) => patchSlotConfig(selectedSlot, { city: e.target.value })}
                />
              </label>
              <label className="field">
                <span>UTC Offset (minutes)</span>
                <input
                  type="number"
                  value={Number(activeCfg.utcOffsetMinutes ?? 540)}
                  onChange={(e) => patchSlotConfig(selectedSlot, { utcOffsetMinutes: Number(e.target.value) })}
                />
              </label>
            </div>
          )}
          {activeType === 17 && (
            <label className="field">
              <span>Pet Name</span>
              <input
                type="text"
                value={String(activeCfg.petName ?? "Mochi")}
                onChange={(e) => patchSlotConfig(selectedSlot, { petName: e.target.value })}
              />
            </label>
          )}
          {activeType === 18 && (
            <div className="row-2">
              <label className="field">
                <span>Generic Tile Title</span>
                <input
                  type="text"
                  value={String(activeCfg.title ?? "FPS")}
                  onChange={(e) => patchSlotConfig(selectedSlot, { title: e.target.value })}
                />
              </label>
              <label className="field">
                <span>Subtitle</span>
                <input
                  type="text"
                  value={String(activeCfg.subtitle ?? "LIVE")}
                  onChange={(e) => patchSlotConfig(selectedSlot, { subtitle: e.target.value })}
                />
              </label>
            </div>
          )}
          {![2, 10, 11, 12, 13, 14, 15, 17, 18].includes(activeType) && (
            <p className="muted" style={{ margin: 0 }}>
              {ALL_MOSAIC_MODULES.find((m) => m.typeId === activeType)?.description} — updates live automatically on the
              keyboard display.
            </p>
          )}
        </div>
      </div>

      <aside className="modules-rail" aria-label="Module Library">
        <span className="rail-title">MODULES (ASSIGN TO SLOT {selectedSlot + 1})</span>
        <div className="module-palette-list">
          {consolidatedLibrary.map((mod) => {
            const isSelected = mod.typeId === 12 ? isClockActive : activeType === mod.typeId;
            return (
              <button
                key={mod.typeId}
                type="button"
                draggable
                aria-pressed={isSelected}
                onDragStart={(e) => {
                  setDragTypeId(mod.typeId);
                  e.dataTransfer.setData(MODULE_DRAG_TYPE, String(mod.typeId));
                }}
                onDragEnd={() => setDragTypeId(null)}
                onClick={() => assignSlot(selectedSlot, mod.typeId === 12 && isClockActive ? activeType : mod.typeId)}
                className={`module-card ${isSelected ? "selected" : ""}`}
              >
                <span className="module-card-head">
                  <strong>{mod.name}</strong>
                  <span className="pill">{mod.category}</span>
                </span>
                <small className="muted">{mod.description}</small>
              </button>
            );
          })}
        </div>
      </aside>
    </div>
  );
};
