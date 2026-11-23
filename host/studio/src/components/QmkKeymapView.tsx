import type React from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { v2Client } from "../api/v2Client";
import {
  ANALOG_GAMEPAD_KEYS,
  type ActionSlotInfo,
  decodeQmkKeycode,
  domCodeToHid,
  encodeLayerOp,
  encodeLayerTap,
  encodeModTap,
  HOLD_MOD_OPTIONS,
  KEY_PICKER_GROUPS,
  LAYER_OP_OPTIONS,
  MATRIX_COLS,
  MATRIX_ROWS,
  vialReadAllLayers,
  vialSetKeycode,
} from "../api/vialProtocol";
import { KeyboardSurface, type SurfaceKeyGeom } from "./KeyboardSurface";

interface QmkKeymapViewProps {
  keys: SurfaceKeyGeom[];
  initialKey?: number | null;
  onSynced?: () => void;
}

type EditorMode = "key" | "hold_tap" | "layer" | "action" | "gamepad";

const LAYER_NAMES = ["Base", "Fn", "Gaming", "Layer 3", "Layer 4", "Layer 5", "Layer 6", "Layer 7"];

export const QmkKeymapView: React.FC<QmkKeymapViewProps> = ({ keys, initialKey, onSynced }) => {
  const [connected, setConnected] = useState<boolean | null>(null);
  const [layerCount, setLayerCount] = useState<number>(6);
  const [keymaps, setKeymaps] = useState<number[][][]>([]);
  const [activeLayer, setActiveLayer] = useState<number>(0);
  const [selectedKey, setSelectedKey] = useState<number>(initialKey ?? 32); // Key W default
  const [editorMode, setEditorMode] = useState<EditorMode>("key");
  const [actionsBySlot, setActionsBySlot] = useState<Record<number, ActionSlotInfo>>({});
  const [search, setSearch] = useState("");
  const [capturing, setCapturing] = useState(false);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [syncingActions, setSyncingActions] = useState(false);

  // Hold/Tap composer state
  const [htType, setHtType] = useState<"mt" | "lt">("mt");
  const [htTapHid, setHtTapHid] = useState<number>(0x04); // A
  const [htHoldMod, setHtHoldMod] = useState<number>(0x01); // LCtrl
  const [htHoldLayer, setHtHoldLayer] = useState<number>(1); // Fn

  // Layer key composer state
  const [layerOpBase, setLayerOpBase] = useState<number>(0x5220); // MO
  const [layerTarget, setLayerTarget] = useState<number>(1);

  useEffect(() => {
    if (initialKey != null) setSelectedKey(initialKey);
  }, [initialKey]);

  const loadActions = useCallback(async () => {
    try {
      const saRes = await v2Client.getBusRetained("system/actions");
      const sa = saRes.messages[0]?.payload as { slots?: Record<string, ActionSlotInfo> } | undefined;
      const map: Record<number, ActionSlotInfo> = {};
      if (sa?.slots) {
        for (const [k, v] of Object.entries(sa.slots)) {
          const s = Number(k);
          map[s] = { ...v, slot: s, keycode: v.keycode ?? 0x7e00 + s };
        }
      }
      setActionsBySlot(map);
    } catch {
      // ignore
    }
  }, []);

  const loadFromKeyboard = useCallback(async () => {
    try {
      const st = await v2Client.vialStatus();
      setConnected(st.connected);
      if (!st.connected) return;
      const res = await vialReadAllLayers(MATRIX_ROWS, MATRIX_COLS);
      setLayerCount(res.layerCount);
      setKeymaps(res.keymaps);
    } catch (err) {
      setConnected(false);
      setStatusNote(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void loadActions();
    void loadFromKeyboard();
  }, [loadActions, loadFromKeyboard]);

  const selGeom = useMemo(
    () => keys.find((k) => k.index === selectedKey) ?? keys[0],
    [keys, selectedKey],
  );

  const currentKeycode = useMemo(() => {
    if (!selGeom) return 0;
    return keymaps[activeLayer]?.[selGeom.row]?.[selGeom.col] ?? 0;
  }, [keymaps, activeLayer, selGeom]);

  const currentDecoded = useMemo(
    () => decodeQmkKeycode(currentKeycode, actionsBySlot),
    [currentKeycode, actionsBySlot],
  );

  // Sync editor sub-tab naturally to match whatever is bound on the clicked key
  useEffect(() => {
    if (currentDecoded.kind === "gamepad") setEditorMode("gamepad");
    else if (currentDecoded.kind === "action") setEditorMode("action");
    else if (currentDecoded.kind === "layer") setEditorMode("layer");
    else if (currentDecoded.kind === "hold_tap") setEditorMode("hold_tap");
  }, [selectedKey, activeLayer]);

  const assignKeycode = useCallback(
    async (kc: number, labelHint?: string) => {
      if (!selGeom) return;
      if (!connected) {
        setStatusNote("Connect your keyboard via USB to write live QMK bindings.");
        return;
      }
      try {
        await vialSetKeycode(activeLayer, selGeom.row, selGeom.col, kc);
        setKeymaps((prev) => {
          const next = prev.map((lyr) => lyr.map((row) => [...row]));
          if (next[activeLayer]?.[selGeom.row]) {
            next[activeLayer][selGeom.row][selGeom.col] = kc;
          }
          return next;
        });
        const dec = decodeQmkKeycode(kc, actionsBySlot);
        setStatusNote(
          `Saved Key ${selGeom.index} (${LAYER_NAMES[activeLayer] ?? `L${activeLayer}`}) → ${labelHint || dec.label}`,
        );
        onSynced?.();
      } catch (err) {
        setStatusNote(err instanceof Error ? err.message : String(err));
      }
    },
    [selGeom, connected, activeLayer, actionsBySlot, onSynced],
  );

  // Live "Press any key..." capture mode
  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const hid = domCodeToHid(e.code);
      setCapturing(false);
      if (hid != null) {
        void assignKeycode(hid);
      } else {
        setStatusNote(`Unrecognized key "${e.code}" — pick from the list below.`);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing, assignKeycode]);

  const handleSyncActions = async () => {
    setSyncingActions(true);
    setStatusNote(null);
    try {
      const res = await v2Client.vialSyncActions();
      await loadActions();
      setStatusNote(
        res.firmware_synced
          ? `Synced ${res.slot_count} module actions to keyboard firmware.`
          : res.warnings[0] || "Saved actions locally (keyboard offline).",
      );
    } catch (err) {
      setStatusNote(err instanceof Error ? err.message : String(err));
    } finally {
      setSyncingActions(false);
    }
  };

  const actionList = useMemo(() => {
    return Object.values(actionsBySlot)
      .filter((a) => a.slot < 39) // slots 0..13 built-in + 14..38 dynamic module actions (39..63 are Gamepad)
      .sort((a, b) => a.slot - b.slot);
  }, [actionsBySlot]);

  const filteredGroups = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return KEY_PICKER_GROUPS;
    return KEY_PICKER_GROUPS.map((g) => ({
      ...g,
      keys: g.keys.filter((k) => k.label.toLowerCase().includes(q)),
    })).filter((g) => g.keys.length > 0);
  }, [search]);

  return (
    <div className="kg-module-page">
      {/* Calm Module Header (5b aesthetic) */}
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>QMK Keymap</h1>
          <span className="kg-pill-subtle">Module · Native Vial</span>
          <span className={`kg-status-badge ${connected ? "is-ok" : "is-warn"}`}>
            {connected === null ? "Checking…" : connected ? "Live EEPROM" : "Keyboard Offline"}
          </span>
        </div>
        <div className="kg-header-actions">
          <button
            type="button"
            className="kg-btn-ghost"
            disabled={syncingActions}
            onClick={() => void handleSyncActions()}
          >
            {syncingActions ? "Syncing…" : "Sync Module Actions"}
          </button>
          <button type="button" className="kg-btn-ghost" onClick={() => void loadFromKeyboard()}>
            Reload
          </button>
        </div>
      </div>

      {/* Centered Calm Layer Pills (5b aesthetic) */}
      <div className="kg-layer-pills" role="tablist" aria-label="QMK Layers">
        {Array.from({ length: Math.min(layerCount, 8) }, (_, idx) => (
          <button
            key={idx}
            type="button"
            role="tab"
            aria-selected={activeLayer === idx}
            className={`kg-layer-pill ${activeLayer === idx ? "is-active" : ""}`}
            onClick={() => setActiveLayer(idx)}
          >
            {LAYER_NAMES[idx] ?? `L${idx}`}
          </button>
        ))}
      </div>

      {/* Shared Interactive 85-Key Keyboard Centerpiece */}
      <div className="kg-surface-wrap">
        <KeyboardSurface
          keys={keys}
          selectedKey={selectedKey}
          onSelectKey={(k) => setSelectedKey(k)}
          topBarSummary={`Layer ${activeLayer} · ${LAYER_NAMES[activeLayer] ?? `L${activeLayer}`} — Click any key below to edit its QMK binding`}
          renderKey={(k) => {
            const kc = keymaps[activeLayer]?.[k.row]?.[k.col] ?? 0;
            const dec = decodeQmkKeycode(kc, actionsBySlot);
            return {
              label: dec.label,
              sub: dec.sub,
              color: dec.accent || (dec.kind === "trns" ? "#64748b" : "#eef4ff"),
              muted: dec.kind === "trns" || dec.kind === "none",
              dotColor:
                dec.kind === "gamepad"
                  ? "#58f29c"
                  : dec.kind === "action"
                    ? dec.accent || "#fa6831"
                    : dec.kind === "layer" || dec.kind === "hold_tap"
                      ? "#a78bfa"
                      : undefined,
              title: `Key ${k.index} (R${k.row}C${k.col}) · ${dec.label} (0x${kc.toString(16).toUpperCase().padStart(4, "0")})`,
            };
          }}
        />
      </div>

      {/* Single Calm Horizontal Editor Strip Directly Below the Keyboard (5b aesthetic) */}
      <div className="kg-keymap-strip">
        <div className="kg-strip-top">
          <div className="kg-strip-id">
            <strong>Edit Key {selectedKey}</strong>
            <span className="kg-pill-accent">{LAYER_NAMES[activeLayer] ?? `L${activeLayer}`}</span>
            <span className="kg-muted">
              Current: <strong style={{ color: currentDecoded.accent || "#eef4ff" }}>{currentDecoded.label}</strong>
              {currentDecoded.sub ? ` (${currentDecoded.sub})` : ""}
            </span>
          </div>

          <div className="kg-segmented" role="tablist">
            {(
              [
                ["key", "Key"],
                ["hold_tap", "Hold / Tap"],
                ["layer", "Layer"],
                ["action", "Module Action"],
                ["gamepad", "Analog Gamepad"],
              ] as Array<[EditorMode, string]>
            ).map(([m, lbl]) => (
              <button
                key={m}
                type="button"
                className={`kg-seg-btn ${editorMode === m ? "is-active" : ""}`}
                onClick={() => setEditorMode(m)}
              >
                {lbl}
              </button>
            ))}
          </div>
        </div>

        {/* Focused Body for Active Mode */}
        <div className="kg-strip-body">
          {editorMode === "key" && (
            <div className="kg-picker-wrap">
              <div className="kg-picker-toolbar">
                <button
                  type="button"
                  className={capturing ? "kg-btn-primary" : "kg-btn-ghost"}
                  onClick={() => setCapturing((v) => !v)}
                >
                  {capturing ? "Press any key now…" : "Press Key to Bind"}
                </button>
                <input
                  type="text"
                  className="kg-search-inline"
                  placeholder="Filter keys (e.g. F13, Enter, Vol, Esc)…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
              <div className="kg-key-groups">
                {filteredGroups.map((g) => (
                  <div key={g.group} className="kg-key-group">
                    <span className="kg-group-label">{g.group}</span>
                    <div className="kg-chip-grid">
                      {g.keys.map((k) => (
                        <button
                          key={k.code}
                          type="button"
                          className={`kg-kc-chip ${currentKeycode === k.code ? "is-selected" : ""}`}
                          onClick={() => void assignKeycode(k.code, k.label)}
                        >
                          {k.label}
                        </button>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {editorMode === "hold_tap" && (
            <div className="kg-inline-form">
              <label className="kg-field-inline">
                <span>Type</span>
                <select value={htType} onChange={(e) => setHtType(e.target.value as "mt" | "lt")}>
                  <option value="mt">Mod-Tap (Tap key, Hold modifier)</option>
                  <option value="lt">Layer-Tap (Tap key, Hold layer)</option>
                </select>
              </label>
              <label className="kg-field-inline">
                <span>Tap Key</span>
                <select value={htTapHid} onChange={(e) => setHtTapHid(Number(e.target.value))}>
                  {KEY_PICKER_GROUPS.filter((g) => g.group !== "Special" && g.group !== "Media & System")
                    .flatMap((g) => g.keys)
                    .filter((k) => k.code <= 0xff)
                    .map((k) => (
                      <option key={k.code} value={k.code}>
                        {k.label}
                      </option>
                    ))}
                </select>
              </label>
              {htType === "mt" ? (
                <label className="kg-field-inline">
                  <span>Hold Modifier</span>
                  <select value={htHoldMod} onChange={(e) => setHtHoldMod(Number(e.target.value))}>
                    {HOLD_MOD_OPTIONS.map((m) => (
                      <option key={m.bit} value={m.bit}>
                        {m.label}
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <label className="kg-field-inline">
                  <span>Hold Layer</span>
                  <select value={htHoldLayer} onChange={(e) => setHtHoldLayer(Number(e.target.value))}>
                    {Array.from({ length: Math.min(layerCount, 8) }, (_, i) => (
                      <option key={i} value={i}>
                        {LAYER_NAMES[i] ?? `L${i}`}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              <button
                type="button"
                className="kg-btn-primary"
                onClick={() =>
                  void assignKeycode(
                    htType === "mt" ? encodeModTap(htHoldMod, htTapHid) : encodeLayerTap(htHoldLayer, htTapHid),
                  )
                }
              >
                Apply Hold / Tap
              </button>
            </div>
          )}

          {editorMode === "layer" && (
            <div className="kg-inline-form">
              <label className="kg-field-inline">
                <span>Behavior</span>
                <select value={layerOpBase} onChange={(e) => setLayerOpBase(Number(e.target.value))}>
                  {LAYER_OP_OPTIONS.map((op) => (
                    <option key={op.base} value={op.base}>
                      {op.name} — {op.desc}
                    </option>
                  ))}
                </select>
              </label>
              <label className="kg-field-inline">
                <span>Target Layer</span>
                <select value={layerTarget} onChange={(e) => setLayerTarget(Number(e.target.value))}>
                  {Array.from({ length: Math.min(layerCount, 8) }, (_, i) => (
                    <option key={i} value={i}>
                      {LAYER_NAMES[i] ?? `L${i}`}
                    </option>
                  ))}
                </select>
              </label>
              <button
                type="button"
                className="kg-btn-primary"
                onClick={() => void assignKeycode(encodeLayerOp(layerOpBase, layerTarget))}
              >
                Apply Layer Key
              </button>
            </div>
          )}

          {editorMode === "action" && (
            <div className="kg-picker-wrap">
              <div className="kg-chip-grid">
                {actionList.map((a) => {
                  const lbl = a.label || a.short_label || a.action_id;
                  return (
                    <button
                      key={a.slot}
                      type="button"
                      className={`kg-kc-chip kg-kc-wide ${currentKeycode === a.keycode ? "is-selected" : ""}`}
                      style={{ borderColor: a.accent ? `${a.accent}66` : undefined }}
                      onClick={() => void assignKeycode(a.keycode, lbl)}
                    >
                      <span>{lbl}</span>
                      <small className="kg-muted">
                        {a.module_id ? `${a.module_id} · ${a.mode ?? "tap"}` : "Built-in"}
                      </small>
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {editorMode === "gamepad" && (
            <div className="kg-picker-wrap">
              <div className="kg-muted" style={{ marginBottom: 8, fontSize: 12 }}>
                Bind any Hall-effect key to an analog stick, trigger, or button — enable Classic HID / XInput in the{" "}
                <strong>Analog Gamepad</strong> module to stream at 1000 Hz.
              </div>
              <div className="kg-chip-grid">
                {ANALOG_GAMEPAD_KEYS.map((g) => (
                  <button
                    key={g.code}
                    type="button"
                    className={`kg-kc-chip kg-kc-wide ${currentKeycode === g.code ? "is-selected" : ""}`}
                    onClick={() => void assignKeycode(g.code, g.name)}
                  >
                    <span style={{ color: "#58f29c" }}>{g.label}</span>
                    <small className="kg-muted">{g.name}</small>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>

        {statusNote && <div className="kg-strip-status">{statusNote}</div>}
      </div>
    </div>
  );
};
