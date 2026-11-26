import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type ActiveProfileState, type ProfileEntry, v2Client } from "../api/v2Client";
import {
  ANALOG_GAMEPAD_KEYS,
  type ActionSlotInfo,
  decodeQmkKeycode,
  domCodeToHid,
  encodeLayerOp,
  encodeLayerTap,
  encodeModifiedKey,
  encodeModTap,
  encodeTapDance,
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
  onOpenProfile?: (profileId: string) => void;
}

type EditorMode = "key" | "combo" | "hold_tap" | "layer" | "action" | "gamepad";

const LAYER_NAMES = Array.from({ length: 16 }, (_, i) => (i === 0 ? "Base" : `Layer ${i}`));

/** Standard ANSI A..Z letter positions (row, col, HID keycode) for 1-click "Fill Plain QWERTY". */
const PLAIN_QWERTY_LETTERS: Array<[number, number, number]> = [
  // Row 2: Q..P
  [2, 1, 0x14], [2, 2, 0x1a], [2, 3, 0x08], [2, 4, 0x15], [2, 5, 0x17],
  [2, 6, 0x1c], [2, 7, 0x18], [2, 8, 0x0c], [2, 9, 0x12], [2, 10, 0x13],
  // Row 3: A..L + ;
  [3, 1, 0x04], [3, 2, 0x16], [3, 3, 0x07], [3, 4, 0x09], [3, 5, 0x0a],
  [3, 6, 0x0b], [3, 7, 0x0d], [3, 8, 0x0e], [3, 9, 0x0f], [3, 10, 0x33],
  // Row 4: Z..M
  [4, 1, 0x1d], [4, 2, 0x1b], [4, 3, 0x06], [4, 4, 0x19], [4, 5, 0x05],
  [4, 6, 0x11], [4, 7, 0x10],
];

const TAP_KEY_CHOICES = KEY_PICKER_GROUPS.filter(
  (g) => g.group !== "Special" && g.group !== "Media & System",
)
  .flatMap((g) => g.keys)
  .filter((k) => k.code <= 0xff);

export const QmkKeymapView: React.FC<QmkKeymapViewProps> = ({
  keys,
  initialKey,
  onSynced,
  onOpenProfile,
}) => {
  const [connected, setConnected] = useState<boolean | null>(null);
  const [layerCount, setLayerCount] = useState<number>(8);
  const [keymaps, setKeymaps] = useState<number[][][]>([]);
  const [activeLayer, setActiveLayer] = useState<number>(0);
  const [selectedKey, setSelectedKey] = useState<number>(initialKey ?? 33); // Key W default
  const [editorMode, setEditorMode] = useState<EditorMode>("key");
  const [actionsBySlot, setActionsBySlot] = useState<Record<number, ActionSlotInfo>>({});
  // Declared actions that have no slot (so can't be bound yet), and the shared module slot budget.
  const [unassignedActions, setUnassignedActions] = useState<
    { action_id: string; module_id: string; label: string; reason: string; detail?: string }[]
  >([]);
  const [moduleSlots, setModuleSlots] = useState<{ total: number; used: number } | null>(null);
  const [profiles, setProfiles] = useState<ProfileEntry[]>([]);
  const [manualProfileId, setManualProfileId] = useState<string | null>(null);
  const [activeProfile, setActiveProfile] = useState<ActiveProfileState | null>(null);
  const [layerNames, setLayerNames] = useState<Record<string, string>>({});
  const [layerNameDraft, setLayerNameDraft] = useState<string>("");
  const [badgeEnabled, setBadgeEnabled] = useState<boolean>(false);
  const [badgeHideBase, setBadgeHideBase] = useState<boolean>(false);
  const [badgePos, setBadgePos] = useState<{ x: number; y: number }>({ x: 0.9, y: 0.06 });
  const [draggingBadge, setDraggingBadge] = useState<boolean>(false);
  const surfaceBoxRef = useRef<HTMLDivElement | null>(null);
  const [search, setSearch] = useState("");
  const [capturing, setCapturing] = useState(false);
  const [fillingQwerty, setFillingQwerty] = useState(false);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [syncingActions, setSyncingActions] = useState(false);

  // Mod+Key combo composer state (e.g. Cmd+C, Ctrl+Shift+Cmd+4)
  const [comboMods, setComboMods] = useState<{ ctrl: boolean; shift: boolean; opt: boolean; cmd: boolean }>({
    ctrl: false,
    shift: false,
    opt: false,
    cmd: true,
  });
  const [comboBaseHid, setComboBaseHid] = useState<number>(0x06); // C

  // Tap/Hold composer state (MT / LT)
  const [htType, setHtType] = useState<"mt" | "lt">("lt");
  const [htTapHid, setHtTapHid] = useState<number>(0x2c); // Space
  const [htHoldMod, setHtHoldMod] = useState<number>(0x01); // LCtrl
  const [htHoldLayer, setHtHoldLayer] = useState<number>(1); // Fn

  // Layer switch composer state (MO / TG / DF / OSL / TT)
  const [layerOpBase, setLayerOpBase] = useState<number>(0x5220); // MO
  const [layerTarget, setLayerTarget] = useState<number>(1);

  useEffect(() => {
    if (initialKey != null) setSelectedKey(initialKey);
  }, [initialKey]);

  const loadProfiles = useCallback(async () => {
    try {
      const [res, lnRes, modsRes] = await Promise.all([
        v2Client.getProfiles(),
        v2Client.getLayerNames().catch(() => ({ layer_names: {} })),
        v2Client.listModules().catch(() => ({ modules: [] })),
      ]);
      const kmMod = modsRes.modules.find((m) => m.id === "keys-keymap") ?? null;
      setProfiles(res.profiles ?? []);
      setManualProfileId(res.manual_profile_id ?? null);
      setActiveProfile(res.active ?? null);
      setLayerNames(lnRes.layer_names ?? {});
      if (kmMod?.state?.config && !draggingBadge) {
        const cfg = kmMod.state.config;
        setBadgeEnabled(Boolean(cfg.layer_badge_enabled));
        setBadgeHideBase(Boolean(cfg.layer_badge_hide_base));
        const bx = typeof cfg.layer_badge_x === "number" ? cfg.layer_badge_x : 0.9;
        const by = typeof cfg.layer_badge_y === "number" ? cfg.layer_badge_y : 0.06;
        setBadgePos({ x: bx, y: by });
      }
    } catch {
      // ignore
    }
  }, [draggingBadge]);

  useEffect(() => {
    void loadProfiles();
    const id = window.setInterval(() => void loadProfiles(), 3000);
    return () => window.clearInterval(id);
  }, [loadProfiles]);

  useEffect(() => {
    setLayerNameDraft(layerNames[String(activeLayer)] ?? "");
  }, [activeLayer, layerNames]);

  const layerDisplayName = useCallback(
    (idx: number) => {
      const custom = layerNames[String(idx)];
      if (custom && custom.trim().length > 0) return custom.trim();
      const linked = profiles.filter((p) => p.enabled && p.qmk_layer === idx);
      if (linked.length > 0) return linked.map((p) => p.name).join(" / ");
      return LAYER_NAMES[idx] ?? `Layer ${idx}`;
    },
    [layerNames, profiles],
  );

  const handleSaveLayerName = async (idx: number, rawName: string) => {
    const trimmed = rawName.trim();
    const next = { ...layerNames };
    if (trimmed) {
      next[String(idx)] = trimmed;
    } else {
      delete next[String(idx)];
    }
    setLayerNames(next);
    try {
      await v2Client.putLayerNames(next);
      setStatusNote(trimmed ? `Renamed Layer ${idx} to "${trimmed}".` : `Cleared custom name for Layer ${idx}.`);
      onSynced?.();
    } catch (err) {
      setStatusNote(err instanceof Error ? err.message : String(err));
    }
  };

  const handleToggleProfileLayerLink = async (profileId: string, assignToActiveLayer: boolean) => {
    const nextProfiles = profiles.map((p) =>
      p.id === profileId ? { ...p, qmk_layer: assignToActiveLayer ? activeLayer : null } : p,
    );
    setProfiles(nextProfiles);
    try {
      await v2Client.putProfiles(manualProfileId, nextProfiles);
      await loadProfiles();
      const pName = profiles.find((p) => p.id === profileId)?.name ?? profileId;
      setStatusNote(
        assignToActiveLayer
          ? `Set Layer ${activeLayer} as the default layer for "${pName}".`
          : `Cleared default layer override on "${pName}".`,
      );
      onSynced?.();
    } catch (err) {
      setStatusNote(err instanceof Error ? err.message : String(err));
    }
  };

  const persistLayerBadgeConfig = async (patch: Record<string, unknown>) => {
    try {
      const list = await v2Client.listModules();
      const mod = list.modules.find((m) => m.id === "keys-keymap");
      if (!mod) return;
      const nextCfg = { ...(mod.state.config ?? {}), ...patch };
      await v2Client.updateModuleState("keys-keymap", { enabled: mod.state.enabled, config: nextCfg });
      onSynced?.();
    } catch (err) {
      setStatusNote(err instanceof Error ? err.message : String(err));
    }
  };

  const profilesByLayer = useMemo(() => {
    const map: Record<number, ProfileEntry[]> = {};
    for (const p of profiles) {
      if (p.enabled && typeof p.qmk_layer === "number") {
        if (!map[p.qmk_layer]) map[p.qmk_layer] = [];
        map[p.qmk_layer].push(p);
      }
    }
    return map;
  }, [profiles]);

  const handleFillPlainQwerty = async () => {
    if (!connected) {
      setStatusNote("Connect your keyboard via USB to write Layer keycodes.");
      return;
    }
    if (
      !window.confirm(
        `Fill Layer ${activeLayer} letters (Q–P, A–;, Z–M) with plain QWERTY (no mod-taps or layer-taps)?\n\nAll other keys on Layer ${activeLayer} remain unchanged.`,
      )
    ) {
      return;
    }
    setFillingQwerty(true);
    setStatusNote(`Writing ${PLAIN_QWERTY_LETTERS.length} plain QWERTY keys to Layer ${activeLayer}…`);
    try {
      for (const [r, c, kc] of PLAIN_QWERTY_LETTERS) {
        await vialSetKeycode(activeLayer, r, c, kc);
      }
      setKeymaps((prev) => {
        const next = prev.map((lyr) => lyr.map((row) => [...row]));
        for (const [r, c, kc] of PLAIN_QWERTY_LETTERS) {
          if (next[activeLayer]?.[r]) next[activeLayer][r][c] = kc;
        }
        return next;
      });
      setStatusNote(`Filled Layer ${activeLayer} with plain QWERTY (no mod-taps/layer-taps).`);
      onSynced?.();
    } catch (err) {
      setStatusNote(err instanceof Error ? err.message : String(err));
    } finally {
      setFillingQwerty(false);
    }
  };

  const loadActions = useCallback(async () => {
    try {
      const saRes = await v2Client.getBusRetained("system/actions");
      const sa = saRes.messages[0]?.payload as
        | {
            slots?: Record<string, ActionSlotInfo>;
            unassigned?: { action_id: string; module_id: string; label: string; reason: string; detail?: string }[];
            module_slots?: { total: number; used: number };
          }
        | undefined;
      setUnassignedActions(sa?.unassigned ?? []);
      setModuleSlots(sa?.module_slots ?? null);
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

  // Pre-populate Hold/Tap, Layer, or Combo composers when clicking an existing key of that type
  useEffect(() => {
    if (currentKeycode >= 0x2000 && currentKeycode <= 0x3fff) {
      setHtType("mt");
      setHtHoldMod((currentKeycode >> 8) & 0x1f);
      setHtTapHid(currentKeycode & 0xff);
      setEditorMode("hold_tap");
    } else if (currentKeycode >= 0x4000 && currentKeycode <= 0x4fff) {
      setHtType("lt");
      setHtHoldLayer((currentKeycode >> 8) & 0x0f);
      setHtTapHid(currentKeycode & 0xff);
      setEditorMode("hold_tap");
    } else if (currentKeycode >= 0x5220 && currentKeycode <= 0x52ff) {
      setLayerOpBase(currentKeycode & 0xffe0);
      setLayerTarget(currentKeycode & 0x1f);
      setEditorMode("layer");
    } else if (currentKeycode >= 0x7e27 && currentKeycode <= 0x7e3f) {
      setEditorMode("gamepad");
    } else if (currentKeycode >= 0x7e00 && currentKeycode <= 0x7fff && currentKeycode !== 0x7e5f) {
      setEditorMode("action");
    } else if (currentKeycode >= 0x0100 && currentKeycode <= 0x1fff) {
      const m = (currentKeycode >> 8) & 0x1f;
      setComboMods({
        ctrl: Boolean(m & 1),
        shift: Boolean(m & 2),
        opt: Boolean(m & 4),
        cmd: Boolean(m & 8),
      });
      setComboBaseHid(currentKeycode & 0xff);
      setEditorMode("combo");
    }
  }, [selectedKey, activeLayer, currentKeycode]);

  const assignKeycode = useCallback(
    async (kc: number, labelHint?: string, targetKeyIndex?: number) => {
      const targetGeom =
        targetKeyIndex != null ? keys.find((k) => k.index === targetKeyIndex) ?? selGeom : selGeom;
      if (!targetGeom) return;
      if (targetKeyIndex != null) {
        setSelectedKey(targetKeyIndex);
      }
      if (!connected) {
        setStatusNote("Connect your keyboard via USB to write live QMK bindings.");
        return;
      }
      try {
        await vialSetKeycode(activeLayer, targetGeom.row, targetGeom.col, kc);
        setKeymaps((prev) => {
          const next = prev.map((lyr) => lyr.map((row) => [...row]));
          if (next[activeLayer]?.[targetGeom.row]) {
            next[activeLayer][targetGeom.row][targetGeom.col] = kc;
          }
          return next;
        });
        const dec = decodeQmkKeycode(kc, actionsBySlot);
        setStatusNote(
          `Saved Key ${targetGeom.index} (${LAYER_NAMES[activeLayer] ?? `L${activeLayer}`}) → ${labelHint || dec.label} (0x${kc.toString(16).toUpperCase().padStart(4, "0")})`,
        );
        onSynced?.();
      } catch (err) {
        setStatusNote(err instanceof Error ? err.message : String(err));
      }
    },
    [keys, selGeom, connected, activeLayer, actionsBySlot, onSynced],
  );

  const makeDragProps = (kc: number, label: string) => ({
    draggable: true,
    onDragStart: (e: React.DragEvent) => {
      e.dataTransfer.setData("application/x-keyglass-keycode", JSON.stringify({ kc, label }));
      e.dataTransfer.setData("text/plain", label);
      e.dataTransfer.effectAllowed = "copy";
    },
    title: `Click to assign to Key ${selectedKey}, or drag onto any keycap above`,
  });

  const handleDropOnKey = useCallback(
    (keyIndex: number, dt: DataTransfer) => {
      const raw = dt.getData("application/x-keyglass-keycode");
      if (!raw) return;
      try {
        const parsed = JSON.parse(raw) as { kc?: number; label?: string };
        if (typeof parsed.kc === "number") {
          void assignKeycode(parsed.kc, parsed.label, keyIndex);
        }
      } catch {
        // ignore invalid drop payload
      }
    },
    [assignKeycode],
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
        setStatusNote(`Unrecognized key "${e.code}" — pick from the grid below.`);
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
      // Built-in (0..13) and module actions (14..38, 64..511 except 95); 39..63 are gamepad codes.
      .filter((a) => a.slot < 39 || (a.slot >= 64 && a.slot <= 511 && a.slot !== 95))
      .sort((a, b) => a.slot - b.slot);
  }, [actionsBySlot]);

  const filteredGroups = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return KEY_PICKER_GROUPS;
    const base = KEY_PICKER_GROUPS.map((g) => ({
      ...g,
      keys: g.keys.filter((k) => k.label.toLowerCase().includes(q)),
    })).filter((g) => g.keys.length > 0);

    // Oryx-style unified search: typing "prusa", "layer", "stick", "pomo", etc. surfaces ALL categories!
    const layerHits = Array.from({ length: Math.min(layerCount, 16) }, (_, i) => ({
      code: encodeLayerOp(0x5220, i),
      label: `Hold L${i} (${layerDisplayName(i)})`,
    })).filter((k) => k.label.toLowerCase().includes(q));
    if (layerHits.length > 0) base.push({ group: "Layers (Hold MO)", keys: layerHits });

    const actHits = actionList
      .map((a) => ({ code: a.keycode, label: a.label || a.short_label || a.action_id }))
      .filter((k) => k.label.toLowerCase().includes(q));
    if (actHits.length > 0) base.push({ group: "Module Actions", keys: actHits });

    const gpHits = ANALOG_GAMEPAD_KEYS.filter(
      (g) => g.name.toLowerCase().includes(q) || g.label.toLowerCase().includes(q),
    ).map((g) => ({ code: g.code, label: `${g.label} · ${g.name}` }));
    if (gpHits.length > 0) base.push({ group: "Analog Gamepad", keys: gpHits });

    return base;
  }, [search, layerCount, actionList]);

  // If the selected key points to another layer (MO/TG/DF/OSL/TT or LT), offer Oryx's 1-click jump!
  const linkedLayerTarget = useMemo(() => {
    if (currentKeycode >= 0x4000 && currentKeycode <= 0x4fff) return (currentKeycode >> 8) & 0x0f;
    if (currentKeycode >= 0x5220 && currentKeycode <= 0x52ff) return currentKeycode & 0x1f;
    return null;
  }, [currentKeycode]);

  const tapKeyLabel = TAP_KEY_CHOICES.find((k) => k.code === htTapHid)?.label ?? "Key";
  const holdModLabel = HOLD_MOD_OPTIONS.find((m) => m.bit === htHoldMod)?.label ?? "Mod";
  const comboMask =
    (comboMods.ctrl ? 1 : 0) |
    (comboMods.shift ? 2 : 0) |
    (comboMods.opt ? 4 : 0) |
    (comboMods.cmd ? 8 : 0);

  return (
    <div className="kg-module-page">
      {/* Calm Module Header */}
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>QMK Keymap</h1>
          <span className="kg-pill-subtle">Native Live EEPROM</span>
          <span className={`kg-status-badge ${connected ? "is-ok" : "is-warn"}`}>
            {connected === null ? "Checking…" : connected ? "Live Connected" : "Keyboard Offline"}
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
            Reload from Keyboard
          </button>
        </div>
      </div>

      {/* Layer Switcher Pills (up to 16 layers with custom names & live Profile links!) */}
      <div className="kg-layer-pills" role="tablist" aria-label="QMK Layers">
        {Array.from({ length: Math.min(layerCount, 16) }, (_, idx) => {
          const linkedProfs = profilesByLayer[idx] ?? [];
          const isLiveDefault = activeProfile?.qmk_layer === idx || (activeProfile?.qmk_layer == null && idx === 0);
          const lbl = layerDisplayName(idx);
          return (
            <button
              key={idx}
              type="button"
              role="tab"
              aria-selected={activeLayer === idx}
              className={`kg-layer-pill ${activeLayer === idx ? "is-active" : ""}`}
              onClick={() => setActiveLayer(idx)}
              title={
                linkedProfs.length > 0
                  ? `Layer ${idx} (${lbl}) — Activated by profile: ${linkedProfs.map((p) => p.name).join(", ")}`
                  : `Layer ${idx} (${lbl})`
              }
            >
              {isLiveDefault && (
                <span
                  className="kg-conn-dot is-on"
                  style={{ display: "inline-block", marginRight: 6, verticalAlign: "middle" }}
                  title="Active default layer on this computer"
                />
              )}
              L{idx} · {lbl}
            </button>
          );
        })}
        {activeLayer > 0 && connected && (
          <button
            type="button"
            className="kg-btn-ghost"
            style={{ marginLeft: "auto", padding: "4px 10px", fontSize: 11 }}
            disabled={fillingQwerty}
            onClick={() => void handleFillPlainQwerty()}
            title="Fill Q–P, A–;, Z–M on this layer with plain QWERTY letters (no tap-hold / mod-taps)"
          >
            {fillingQwerty ? "Filling…" : `Fill L${activeLayer} Plain QWERTY`}
          </button>
        )}
      </div>

      {/* Layer Controls Strip: Rename Layer + Assign Profile Default Layer + Draggable On-Keyboard Layer Badge */}
      <div
        className="kg-calm-card kg-calm-row"
        style={{ padding: "8px 14px", flexWrap: "wrap", gap: 10 }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
          <span className="kg-muted" style={{ fontSize: 11 }}>
            Layer {activeLayer} name:
          </span>
          <input
            type="text"
            value={layerNameDraft}
            onChange={(e) => setLayerNameDraft(e.target.value)}
            onBlur={() => {
              if ((layerNames[String(activeLayer)] ?? "") !== layerNameDraft.trim()) {
                void handleSaveLayerName(activeLayer, layerNameDraft);
              }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                void handleSaveLayerName(activeLayer, layerNameDraft);
                (e.target as HTMLInputElement).blur();
              }
            }}
            placeholder={LAYER_NAMES[activeLayer] ?? `Layer ${activeLayer}`}
            style={{ width: 132, padding: "4px 8px", fontSize: 12 }}
          />
          <span className="kg-muted" style={{ fontSize: 11, marginLeft: 4 }}>
            Default for profile:
          </span>
          <select
            value=""
            onChange={(e) => {
              const pid = e.target.value;
              if (!pid) return;
              const existing = profiles.find((p) => p.id === pid);
              void handleToggleProfileLayerLink(pid, existing?.qmk_layer !== activeLayer);
            }}
            style={{ padding: "4px 8px", fontSize: 12 }}
            title="Toggle which Profile automatically selects this layer as its default layer"
          >
            <option value="">
              {(profilesByLayer[activeLayer] ?? []).length > 0
                ? (profilesByLayer[activeLayer] ?? []).map((p) => p.name).join(", ")
                : "None (click to assign…)"}
            </option>
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.qmk_layer === activeLayer ? `✓ ${p.name} (click to clear)` : `Assign to ${p.name}`}
              </option>
            ))}
          </select>
          {(profilesByLayer[activeLayer] ?? []).length > 0 && onOpenProfile && (
            <button
              type="button"
              className="kg-btn-ghost"
              style={{ padding: "2px 8px", fontSize: 11 }}
              onClick={() => onOpenProfile(profilesByLayer[activeLayer][0].id)}
            >
              Open Profile →
            </button>
          )}
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 8, marginLeft: "auto", flexWrap: "wrap" }}>
          <label
            style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12, cursor: "pointer" }}
            title="Show a draggable active layer indicator pill directly on your keyboard display"
          >
            <input
              type="checkbox"
              checked={badgeEnabled}
              onChange={(e) => {
                const on = e.target.checked;
                setBadgeEnabled(on);
                void persistLayerBadgeConfig({ layer_badge_enabled: on });
              }}
            />
            <span>Layer Badge on Keyboard</span>
          </label>
          {badgeEnabled && (
            <>
              <label style={{ display: "inline-flex", alignItems: "center", gap: 5, fontSize: 11, cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={badgeHideBase}
                  onChange={(e) => {
                    const hide = e.target.checked;
                    setBadgeHideBase(hide);
                    void persistLayerBadgeConfig({ layer_badge_hide_base: hide });
                  }}
                />
                <span className="kg-muted">Hide on base</span>
              </label>
              <button
                type="button"
                className="kg-btn-ghost"
                style={{ padding: "2px 7px", fontSize: 11 }}
                onClick={() => {
                  const def = { x: 0.9, y: 0.06 };
                  setBadgePos(def);
                  void persistLayerBadgeConfig({ layer_badge_x: def.x, layer_badge_y: def.y });
                }}
              >
                Reset Pos
              </button>
            </>
          )}
        </div>
      </div>

      {/* Double-Click Quick-Capture Banner */}
      {capturing && (
        <div
          className="kg-calm-card kg-calm-row"
          style={{ borderColor: "rgba(92, 225, 255, 0.55)", background: "rgba(92, 225, 255, 0.08)" }}
        >
          <div style={{ fontSize: 12 }}>
            <strong style={{ color: "#7dd3fc" }}>
              Listening for key press on Key {selectedKey} (L{activeLayer})…
            </strong>{" "}
            <span className="kg-muted">Press any physical key on your keyboard to bind it immediately.</span>
          </div>
          <button type="button" className="kg-btn-ghost" onClick={() => setCapturing(false)}>
            Cancel
          </button>
        </div>
      )}

      {/* Shared Interactive 85-Key Keyboard (with Draggable On-Keyboard Layer Badge!) */}
      <div className="kg-surface-wrap" ref={surfaceBoxRef}>
        <KeyboardSurface
          keys={keys}
          selectedKey={selectedKey}
          onSelectKey={(k) => {
            setSelectedKey(k);
            if (capturing) setCapturing(false);
          }}
          onDoubleClickKey={(k) => {
            setSelectedKey(k);
            setCapturing(true);
            setStatusNote(`Listening on Key ${k} (L${activeLayer}) — press any key to bind it.`);
          }}
          onDropOnKey={handleDropOnKey}
          topBarSummary={
            badgeEnabled
              ? `Editing L${activeLayer} (${layerDisplayName(activeLayer)}) — Drag the glowing Layer Badge pill anywhere on the keyboard`
              : `Editing L${activeLayer} (${layerDisplayName(activeLayer)}) — Click a key, double-click & type to quick-bind, or drag any binding below`
          }
          overlayChildren={
            badgeEnabled ? (
              <div
                role="slider"
                aria-label="Draggable Layer Badge Position"
                aria-valuetext={`${Math.round(badgePos.x * 100)}%, ${Math.round(badgePos.y * 100)}%`}
                tabIndex={0}
                onPointerDown={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  e.currentTarget.setPointerCapture(e.pointerId);
                  setDraggingBadge(true);
                }}
                onPointerMove={(e) => {
                  if (!draggingBadge) return;
                  const surface = e.currentTarget.parentElement;
                  if (!surface) return;
                  const rect = surface.getBoundingClientRect();
                  const nx = Math.max(0.04, Math.min(0.96, (e.clientX - rect.left) / rect.width));
                  const ny = Math.max(0.03, Math.min(0.96, (e.clientY - rect.top) / rect.height));
                  setBadgePos({ x: Number(nx.toFixed(3)), y: Number(ny.toFixed(3)) });
                }}
                onPointerUp={(e) => {
                  if (!draggingBadge) return;
                  e.currentTarget.releasePointerCapture(e.pointerId);
                  setDraggingBadge(false);
                  const surface = e.currentTarget.parentElement;
                  if (!surface) return;
                  const rect = surface.getBoundingClientRect();
                  const nx = Number(Math.max(0.04, Math.min(0.96, (e.clientX - rect.left) / rect.width)).toFixed(3));
                  const ny = Number(Math.max(0.03, Math.min(0.96, (e.clientY - rect.top) / rect.height)).toFixed(3));
                  setBadgePos({ x: nx, y: ny });
                  void persistLayerBadgeConfig({ layer_badge_x: nx, layer_badge_y: ny });
                }}
                style={{
                  position: "absolute",
                  left: `${badgePos.x * 100}%`,
                  top: `${badgePos.y * 100}%`,
                  transform: "translate(-50%, -50%)",
                  zIndex: 25,
                  cursor: draggingBadge ? "grabbing" : "grab",
                  userSelect: "none",
                  padding: "4px 12px",
                  borderRadius: 999,
                  background: "rgba(10, 14, 24, 0.88)",
                  border: "1.5px solid rgba(92, 225, 255, 0.8)",
                  boxShadow: draggingBadge
                    ? "0 0 20px rgba(92, 225, 255, 0.55)"
                    : "0 4px 14px rgba(0, 0, 0, 0.55)",
                  color: "#eef6ff",
                  fontSize: 12,
                  fontWeight: 600,
                  display: "inline-flex",
                  alignItems: "center",
                  gap: 6,
                  whiteSpace: "nowrap",
                }}
                title="Drag anywhere on the keyboard to position the live Layer Badge"
              >
                <span
                  style={{
                    width: 7,
                    height: 7,
                    borderRadius: "50%",
                    background: "#5ce1ff",
                    display: "inline-block",
                  }}
                />
                L{activeLayer} · {layerDisplayName(activeLayer)}
              </div>
            ) : null
          }
          renderKey={(k) => {
            const isCap = capturing && selectedKey === k.index;
            if (isCap) {
              return {
                label: "Type…",
                sub: "listen",
                color: "#5ce1ff",
                border: "rgba(92, 225, 255, 0.9)",
                bg: "rgba(92, 225, 255, 0.16)",
                title: `Listening for physical key press on Key ${k.index}`,
              };
            }
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
              title: `Key ${k.index} (R${k.row} C${k.col}) · ${dec.label} (0x${kc.toString(16).toUpperCase().padStart(4, "0")}) — Double-click to quick-bind`,
            };
          }}
        />
      </div>

      {/* Focused Single-Key Binding Strip */}
      <div className="kg-keymap-strip">
        <div className="kg-strip-top">
          <div className="kg-strip-id">
            <strong>Key {selectedKey}</strong>
            {selGeom && (
              <span className="kg-muted">
                (R{selGeom.row} C{selGeom.col})
              </span>
            )}
            <span className="kg-pill-accent">{LAYER_NAMES[activeLayer] ?? `L${activeLayer}`}</span>
            <span className="kg-muted">
              Bound to:{" "}
              <strong style={{ color: currentDecoded.accent || "#eef4ff" }}>{currentDecoded.label}</strong>
              {currentDecoded.sub ? ` (${currentDecoded.sub})` : ""}{" "}
              <code style={{ opacity: 0.65 }}>
                0x{currentKeycode.toString(16).toUpperCase().padStart(4, "0")}
              </code>
            </span>
            {linkedLayerTarget != null && linkedLayerTarget !== activeLayer && (
              <button
                type="button"
                className="kg-btn-ghost"
                onClick={() => setActiveLayer(linkedLayerTarget)}
              >
                Edit {LAYER_NAMES[linkedLayerTarget] ?? `L${linkedLayerTarget}`} →
              </button>
            )}
          </div>

          <div className="kg-segmented" role="tablist">
            {(
              [
                ["key", "Basic Key"],
                ["combo", "Mod + Key"],
                ["hold_tap", "Tap / Hold"],
                ["layer", "Layer Switch"],
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

        <div className="kg-strip-body">
          {/* 1. Basic Key */}
          {editorMode === "key" && (
            <div className="kg-picker-wrap">
              <div className="kg-picker-toolbar">
                <button
                  type="button"
                  className={capturing ? "kg-btn-primary" : "kg-btn-ghost"}
                  onClick={() => setCapturing((v) => !v)}
                >
                  {capturing ? "Press any key now…" : "⌨ Press Key to Bind"}
                </button>
                <input
                  type="text"
                  className="kg-search-inline"
                  placeholder="Search keys (e.g. Esc, F13, Enter, Vol, Del, Trans)…"
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
                          {...makeDragProps(k.code, k.label)}
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

          {/* 2. Modified Shortcut Key (e.g. Cmd+C, Ctrl+Shift+Cmd+4, Shift+Symbol) */}
          {editorMode === "combo" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              <div className="kg-muted">
                Emit a key combined with modifiers in one press (e.g. <strong>⌃⇧⌘4</strong> screenshot,{" "}
                <strong>⌘C</strong> copy, or <strong>⇧Tab</strong>).
              </div>
              <div className="kg-inline-form">
                <div style={{ display: "flex", gap: 12, alignItems: "center", paddingBottom: 6 }}>
                  {(
                    [
                      ["ctrl", "⌃ Ctrl"],
                      ["shift", "⇧ Shift"],
                      ["opt", "⌥ Opt"],
                      ["cmd", "⌘ Cmd"],
                    ] as Array<[keyof typeof comboMods, string]>
                  ).map(([k, lbl]) => (
                    <label key={k} style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 13 }}>
                      <input
                        type="checkbox"
                        checked={comboMods[k]}
                        onChange={(e) => setComboMods((p) => ({ ...p, [k]: e.target.checked }))}
                      />
                      {lbl}
                    </label>
                  ))}
                </div>
                <label className="kg-field-inline">
                  <span>Key</span>
                  <select value={comboBaseHid} onChange={(e) => setComboBaseHid(Number(e.target.value))}>
                    {TAP_KEY_CHOICES.map((k) => (
                      <option key={k.code} value={k.code}>
                        {k.label}
                      </option>
                    ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="kg-btn-primary"
                  disabled={comboMask === 0}
                  {...(comboMask !== 0 ? makeDragProps(encodeModifiedKey(comboMask, comboBaseHid), "Mod+Key") : {})}
                  onClick={() => void assignKeycode(encodeModifiedKey(comboMask, comboBaseHid))}
                >
                  Assign (or Drag) Mod + Key
                </button>
              </div>
            </div>
          )}

          {/* 3. Tap / Hold (Mod-Tap & Layer-Tap) + Tap Dance */}
          {editorMode === "hold_tap" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              <div className="kg-inline-form">
                <label className="kg-field-inline">
                  <span>When TAPPED, send key:</span>
                  <select value={htTapHid} onChange={(e) => setHtTapHid(Number(e.target.value))}>
                    {TAP_KEY_CHOICES.map((k) => (
                      <option key={k.code} value={k.code}>
                        {k.label}
                      </option>
                    ))}
                  </select>
                </label>

                <label className="kg-field-inline">
                  <span>When HELD, act as:</span>
                  <select value={htType} onChange={(e) => setHtType(e.target.value as "mt" | "lt")}>
                    <option value="lt">Hold a Layer (Layer-Tap)</option>
                    <option value="mt">Hold a Modifier (Mod-Tap)</option>
                  </select>
                </label>

                {htType === "mt" ? (
                  <label className="kg-field-inline">
                    <span>Held Modifier</span>
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
                    <span>Held Layer</span>
                    <select value={htHoldLayer} onChange={(e) => setHtHoldLayer(Number(e.target.value))}>
                      {Array.from({ length: Math.min(layerCount, 16) }, (_, i) => (
                        <option key={i} value={i}>
                          L{i} · {layerDisplayName(i)}
                        </option>
                      ))}
                    </select>
                  </label>
                )}

                <button
                  type="button"
                  className="kg-btn-primary"
                  {...makeDragProps(
                    htType === "mt" ? encodeModTap(htHoldMod, htTapHid) : encodeLayerTap(htHoldLayer, htTapHid),
                    `${tapKeyLabel}/${htType === "mt" ? holdModLabel : `L${htHoldLayer}`}`,
                  )}
                  onClick={() =>
                    void assignKeycode(
                      htType === "mt" ? encodeModTap(htHoldMod, htTapHid) : encodeLayerTap(htHoldLayer, htTapHid),
                    )
                  }
                >
                  Assign (or Drag) Tap / Hold
                </button>
              </div>
              <div className="kg-muted" style={{ fontSize: 12 }}>
                Preview: Tap quickly → sends <strong>{tapKeyLabel}</strong> · Hold down → activates{" "}
                <strong>
                  {htType === "mt" ? holdModLabel : `L${htHoldLayer} (${LAYER_NAMES[htHoldLayer] ?? `Layer ${htHoldLayer}`})`}
                </strong>
              </div>
              <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap", paddingTop: 4 }}>
                <span className="kg-group-label" style={{ marginRight: 6 }}>
                  Tap Dance Slots:
                </span>
                {[0, 1, 2, 3, 4, 5, 6, 7].map((td) => (
                  <button
                    key={td}
                    type="button"
                    className={`kg-kc-chip ${currentKeycode === encodeTapDance(td) ? "is-selected" : ""}`}
                    {...makeDragProps(encodeTapDance(td), `TD(${td})`)}
                    onClick={() => void assignKeycode(encodeTapDance(td), `TD(${td})`)}
                  >
                    TD({td})
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* 4. Layer Switch (MO / TG / DF / OSL / TT) */}
          {editorMode === "layer" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              <div className="kg-chip-grid">
                {LAYER_OP_OPTIONS.map((op) => (
                  <button
                    key={op.base}
                    type="button"
                    className={`kg-kc-chip kg-kc-wide ${layerOpBase === op.base ? "is-selected" : ""}`}
                    onClick={() => setLayerOpBase(op.base)}
                  >
                    <span>{op.name}</span>
                    <small className="kg-muted">{op.desc}</small>
                  </button>
                ))}
              </div>
              <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                <span className="kg-muted">Click a target layer (or drag it onto any key above):</span>
                {Array.from({ length: Math.min(layerCount, 16) }, (_, i) => {
                  const kc = encodeLayerOp(layerOpBase, i);
                  return (
                    <button
                      key={i}
                      type="button"
                      className={`kg-kc-chip ${currentKeycode === kc ? "is-selected" : ""}`}
                      {...makeDragProps(kc, `L${i}`)}
                      onClick={() => {
                        setLayerTarget(i);
                        void assignKeycode(kc);
                      }}
                    >
                      L{i} · {layerDisplayName(i)}
                    </button>
                  );
                })}
              </div>
              <div className="kg-muted" style={{ fontSize: 11 }}>
                Tip: Switch the active layer pills above to edit what keys live on{" "}
                <strong>L{layerTarget} ({LAYER_NAMES[layerTarget] ?? `Layer ${layerTarget}`})</strong>!
              </div>
            </div>
          )}

          {/* 5. Module Actions */}
          {editorMode === "action" && (
            <div className="kg-picker-wrap">
              <div className="kg-chip-grid">
                {actionList.map((a) => {
                  const lbl = a.label || a.short_label || a.action_id;
                  const modName =
                    a.module_id === "openflux"
                      ? "Built-in"
                      : a.module_id === "streamdeck-bridge"
                      ? "Stream Deck"
                      : a.module_id === "widget-prusa-connect"
                        ? "Prusa Printer"
                        : a.module_id === "profiles"
                          ? "Profiles"
                          : a.module_id === "ui-osd-toast"
                            ? "Notifications"
                            : a.module_id || "Built-in";
                  return (
                    <button
                      key={a.slot}
                      type="button"
                      className={`kg-kc-chip kg-kc-wide ${currentKeycode === a.keycode ? "is-selected" : ""}`}
                      style={{ borderColor: a.accent ? `${a.accent}66` : undefined }}
                      {...makeDragProps(a.keycode, lbl)}
                      onClick={() => void assignKeycode(a.keycode, lbl)}
                    >
                      <span>{lbl}</span>
                      <small className="kg-muted">
                        {modName} · {a.mode ?? "tap"}
                      </small>
                    </button>
                  );
                })}
              </div>
              {moduleSlots && (
                <div className="kg-muted" style={{ fontSize: 11, marginTop: 8 }}>
                  Module action slots: {moduleSlots.used} of {moduleSlots.total} in use.
                </div>
              )}
              {unassignedActions.length > 0 && (
                <div className="kg-muted" style={{ fontSize: 11, marginTop: 6 }}>
                  <strong>Not bindable yet ({unassignedActions.length}):</strong>{" "}
                  {unassignedActions
                    .map(
                      (u) =>
                        `${u.label || u.action_id} (${u.module_id}: ${
                          u.reason === "no_free_slot" ? "no free slot" : u.detail || u.reason
                        })`,
                    )
                    .join(", ")}
                </div>
              )}
            </div>
          )}

          {/* 6. Analog Gamepad */}
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
                    {...makeDragProps(g.code, g.name)}
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
