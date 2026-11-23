import type React from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { v2Client } from "../api/v2Client";

interface KeyGeom {
  index: number;
  center_x: number;
  center_y: number;
  rect: { x: number; y: number; w: number; h: number };
}

interface ResolvedKey {
  keycode: number;
  source_layer: number;
}

interface ActionEntry {
  slot: number;
  keycode?: number;
  action_id: string;
  module_id?: string;
  label?: string;
  short_label?: string;
  icon?: string;
  accent?: string;
  mode?: string;
  topic?: string;
}

interface ProviderRule {
  key?: number | string;
  mods?: number;
  priority?: number;
  label?: string;
  shifted?: string;
  icon?: string;
  image?: string;
  color?: string;
  bg?: string;
  border?: string;
}

interface ProviderClaim {
  topic: string;
  priority: number;
  rule: ProviderRule;
}

interface CustomOverride {
  label?: string;
  color?: string;
  bg?: string;
  border?: string;
  icon?: string;
  image?: string;
  hide?: boolean;
}

interface KeycapsTabProps {
  pickFile?: (title: string) => Promise<string | null>;
}

const BUILTIN_ICONS = [
  "",
  "cam",
  "play",
  "refresh",
  "brightness",
  "volume",
  "mic",
  "power",
  "star",
  "copy",
  "paste",
  "cut",
  "undo",
  "redo",
  "save",
  "search",
  "print",
  "close",
  "plus",
  "select_all",
];

function formatHexKeycode(kc: number): string {
  return `0x${kc.toString(16).toUpperCase().padStart(4, "0")}`;
}

export const KeycapsTab: React.FC<KeycapsTabProps> = ({ pickFile }) => {
  const [keys, setKeys] = useState<KeyGeom[]>([]);
  const [resolved, setResolved] = useState<ResolvedKey[]>([]);
  const [activeLayer, setActiveLayer] = useState<number>(0);
  const [actionsByKc, setActionsByKc] = useState<Record<number, ActionEntry>>({});
  const [providersByKey, setProvidersByKey] = useState<Record<number, ProviderClaim[]>>({});
  const [byKeyOverrides, setByKeyOverrides] = useState<Record<string, CustomOverride>>({});
  const [byActionOverrides, setByActionOverrides] = useState<Record<string, CustomOverride>>({});
  const [selectedKey, setSelectedKey] = useState<number>(13);
  const [scope, setScope] = useState<"action" | "key_layer" | "key_all">("key_layer");
  const [draft, setDraft] = useState<CustomOverride>({});
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const loadLiveState = useCallback(async () => {
    try {
      const [geomRes, rkRes, actRes, provRes, modsRes] = await Promise.all([
        v2Client.getBusRetained("keyboard/geometry"),
        v2Client.getBusRetained("keyboard/resolved_keys"),
        v2Client.getBusRetained("system/actions"),
        v2Client.getBusRetained("keys/providers/#"),
        v2Client.listModules(),
      ]);

      const gPayload = geomRes.messages[0]?.payload as { keys?: KeyGeom[] } | undefined;
      if (Array.isArray(gPayload?.keys)) {
        setKeys(gPayload.keys);
      }

      const rkPayload = rkRes.messages[0]?.payload as
        | { active_layer?: number; keys?: ResolvedKey[] }
        | undefined;
      if (typeof rkPayload?.active_layer === "number") {
        setActiveLayer(rkPayload.active_layer);
      }
      if (Array.isArray(rkPayload?.keys)) {
        setResolved(rkPayload.keys);
      }

      const actPayload = actRes.messages[0]?.payload as
        | { slots?: Record<string, ActionEntry> }
        | undefined;
      const actMap: Record<number, ActionEntry> = {};
      if (actPayload?.slots) {
        for (const [slotStr, entry] of Object.entries(actPayload.slots)) {
          const slotNum = Number(slotStr);
          const kc = typeof entry.keycode === "number" ? entry.keycode : 0x7e00 + slotNum;
          actMap[kc] = { ...entry, slot: slotNum, keycode: kc };
        }
      }
      setActionsByKc(actMap);

      const claimsMap: Record<number, ProviderClaim[]> = {};
      for (const msg of provRes.messages) {
        const p = msg.payload as { priority?: number; rules?: ProviderRule[] } | null;
        if (!p || !Array.isArray(p.rules)) continue;
        const basePrio = typeof p.priority === "number" ? p.priority : 10;
        for (const rule of p.rules) {
          // Only show base / non-modifier claims in the static overview
          if (typeof rule.mods === "number" && rule.mods > 0) continue;
          if (typeof rule.key !== "number") continue;
          const prio = typeof rule.priority === "number" ? rule.priority : basePrio;
          const arr = claimsMap[rule.key] ?? [];
          arr.push({ topic: msg.topic.replace("keys/providers/", ""), priority: prio, rule });
          claimsMap[rule.key] = arr;
        }
      }
      for (const k of Object.keys(claimsMap)) {
        claimsMap[Number(k)].sort((a, b) => b.priority - a.priority);
      }
      setProvidersByKey(claimsMap);

      const customMod = modsRes.modules.find((m) => m.id === "keys-custom");
      const cfg = (customMod?.state.config ?? {}) as {
        by_key?: Record<string, CustomOverride>;
        by_action?: Record<string, CustomOverride>;
      };
      setByKeyOverrides(cfg.by_key ?? {});
      setByActionOverrides(cfg.by_action ?? {});
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void loadLiveState();
    const t = window.setInterval(() => void loadLiveState(), 2500);
    return () => window.clearInterval(t);
  }, [loadLiveState]);

  const selectedResolved = resolved[selectedKey];
  const selectedAction = selectedResolved ? actionsByKc[selectedResolved.keycode] : undefined;
  const selectedClaims = providersByKey[selectedKey] ?? [];
  const selectedWinner = selectedClaims[0];

  // Sync draft editor when clicking a new key or changing scope
  useEffect(() => {
    if (selectedAction && byActionOverrides[selectedAction.action_id]) {
      setScope("action");
      setDraft({ ...byActionOverrides[selectedAction.action_id] });
    } else if (byKeyOverrides[`${activeLayer}:${selectedKey}`]) {
      setScope("key_layer");
      setDraft({ ...byKeyOverrides[`${activeLayer}:${selectedKey}`] });
    } else if (byKeyOverrides[`*:${selectedKey}`]) {
      setScope("key_all");
      setDraft({ ...byKeyOverrides[`*:${selectedKey}`] });
    } else {
      setScope(selectedAction ? "action" : "key_layer");
      setDraft({});
    }
  }, [selectedKey, activeLayer, selectedAction?.action_id]);

  const loadDraftForScope = (nextScope: "action" | "key_layer" | "key_all") => {
    setScope(nextScope);
    if (nextScope === "action" && selectedAction) {
      setDraft({ ...(byActionOverrides[selectedAction.action_id] ?? {}) });
    } else if (nextScope === "key_layer") {
      setDraft({ ...(byKeyOverrides[`${activeLayer}:${selectedKey}`] ?? {}) });
    } else {
      setDraft({ ...(byKeyOverrides[`*:${selectedKey}`] ?? {}) });
    }
  };

  const persistOverrides = async (
    nextByKey: Record<string, CustomOverride>,
    nextByAction: Record<string, CustomOverride>,
    msg: string,
  ) => {
    setSaving(true);
    setStatusMsg(null);
    try {
      await v2Client.updateModuleState("keys-custom", {
        enabled: true,
        config: {
          by_key: nextByKey,
          by_action: nextByAction,
        },
      });
      await v2Client.syncToKeyboard().catch(() => {});
      setByKeyOverrides(nextByKey);
      setByActionOverrides(nextByAction);
      setStatusMsg(msg);
      await loadLiveState();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleSave = async () => {
    const cleaned: CustomOverride = {};
    if (draft.label?.trim()) cleaned.label = draft.label.trim();
    if (draft.color?.trim()) cleaned.color = draft.color.trim();
    if (draft.bg?.trim()) cleaned.bg = draft.bg.trim();
    if (draft.border?.trim()) cleaned.border = draft.border.trim();
    if (draft.icon?.trim()) cleaned.icon = draft.icon.trim();
    if (draft.image?.trim()) cleaned.image = draft.image.trim();
    if (draft.hide) cleaned.hide = true;

    const nextByKey = { ...byKeyOverrides };
    const nextByAction = { ...byActionOverrides };

    if (scope === "action" && selectedAction) {
      if (Object.keys(cleaned).length === 0) {
        delete nextByAction[selectedAction.action_id];
      } else {
        nextByAction[selectedAction.action_id] = cleaned;
      }
      await persistOverrides(nextByKey, nextByAction, `Saved visual override for action "${selectedAction.label ?? selectedAction.action_id}" and synced to keyboard.`);
    } else {
      const k = scope === "key_all" ? `*:${selectedKey}` : `${activeLayer}:${selectedKey}`;
      if (Object.keys(cleaned).length === 0) {
        delete nextByKey[k];
      } else {
        nextByKey[k] = cleaned;
      }
      await persistOverrides(nextByKey, nextByAction, `Saved visual override for Key ${selectedKey} and synced to keyboard.`);
    }
  };

  const handleReset = async () => {
    const nextByKey = { ...byKeyOverrides };
    const nextByAction = { ...byActionOverrides };
    if (scope === "action" && selectedAction) {
      delete nextByAction[selectedAction.action_id];
    } else {
      const k = scope === "key_all" ? `*:${selectedKey}` : `${activeLayer}:${selectedKey}`;
      delete nextByKey[k];
    }
    setDraft({});
    await persistOverrides(nextByKey, nextByAction, "Reset keycap override to module/keymap default.");
  };

  const handlePickImage = async () => {
    if (!pickFile) return;
    const localPath = await pickFile("Choose Custom Keycap PNG Icon");
    if (!localPath) return;
    try {
      const res = await v2Client.importMediaFile(localPath);
      setDraft((prev) => ({ ...prev, image: res.name, icon: "" }));
      setStatusMsg(`Imported "${res.name}" — click Apply Override to paint it onto the keyboard.`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const keyBounds = useMemo(() => {
    if (keys.length === 0) return { minX: 0, minY: 260, w: 1920, h: 820 };
    let minX = Infinity;
    let minY = Infinity;
    let maxX = 0;
    let maxY = 0;
    for (const k of keys) {
      minX = Math.min(minX, k.rect.x);
      minY = Math.min(minY, k.rect.y);
      maxX = Math.max(maxX, k.rect.x + k.rect.w);
      maxY = Math.max(maxY, k.rect.y + k.rect.h);
    }
    return { minX, minY, w: Math.max(1, maxX - minX), h: Math.max(1, maxY - minY) };
  }, [keys]);

  return (
    <div style={{ display: "grid", gridTemplateColumns: "minmax(0, 1.55fr) minmax(300px, 1fr)", gap: 14, alignItems: "start" }}>
      {/* Interactive 85-Key Flux Keyboard Map */}
      <div className="card" style={{ padding: 14 }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10 }}>
          <div>
            <h3 style={{ margin: 0 }}>Keycap Visuals &amp; Provider Map</h3>
            <small className="muted">
              Active Layer {activeLayer} · Click any key to inspect its QMK binding, active module owner, or set a custom icon/label override
            </small>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <span className="pill" style={{ fontSize: 11 }}>
              Orange dot = Module Action · Cyan dot = Custom Override
            </span>
            <button type="button" className="btn-secondary" onClick={() => void loadLiveState()}>
              Refresh
            </button>
          </div>
        </div>

        <div
          style={{
            position: "relative",
            width: "100%",
            aspectRatio: `${keyBounds.w} / ${keyBounds.h}`,
            background: "rgba(8, 12, 20, 0.9)",
            borderRadius: 12,
            border: "1px solid rgba(255,255,255,0.1)",
            overflow: "hidden",
          }}
        >
          {keys.map((k) => {
            const claims = providersByKey[k.index] ?? [];
            const win = claims[0];
            const rk = resolved[k.index];
            const act = rk ? actionsByKc[rk.keycode] : undefined;
            const hasOverride =
              Boolean(byKeyOverrides[`${activeLayer}:${k.index}`]) ||
              Boolean(byKeyOverrides[`*:${k.index}`]) ||
              Boolean(act && byActionOverrides[act.action_id]);
            const isSel = selectedKey === k.index;
            const leftPct = ((k.rect.x - keyBounds.minX) / keyBounds.w) * 100;
            const topPct = ((k.rect.y - keyBounds.minY) / keyBounds.h) * 100;
            const wPct = (k.rect.w / keyBounds.w) * 100;
            const hPct = (k.rect.h / keyBounds.h) * 100;

            const dispLabel = win?.rule.label || (win?.rule.image ? "IMG" : win?.rule.icon ? `★` : "");
            const txtColor = win?.rule.color || "#eef4ff";
            const bgColor = win?.rule.bg || (isSel ? "rgba(77, 225, 255, 0.22)" : "rgba(255,255,255,0.055)");

            return (
              <button
                key={k.index}
                type="button"
                onClick={() => setSelectedKey(k.index)}
                title={`Key ${k.index}${act ? ` · ${act.label ?? act.action_id}` : ""}${win ? ` · Painted by ${win.topic} (prio ${win.priority})` : ""}`}
                style={{
                  position: "absolute",
                  left: `calc(${leftPct}% + 2px)`,
                  top: `calc(${topPct}% + 2px)`,
                  width: `calc(${wPct}% - 4px)`,
                  height: `calc(${hPct}% - 4px)`,
                  padding: 2,
                  borderRadius: 6,
                  border: isSel
                    ? "2px solid #4de1ff"
                    : win?.rule.border
                      ? `1px solid ${win.rule.border}`
                      : "1px solid rgba(255,255,255,0.14)",
                  background: bgColor,
                  color: txtColor,
                  fontSize: dispLabel.length > 4 ? 9 : 11,
                  fontWeight: 600,
                  cursor: "pointer",
                  display: "flex",
                  flexDirection: "column",
                  alignItems: "center",
                  justifyContent: "center",
                  overflow: "hidden",
                  lineHeight: 1.1,
                }}
              >
                <span style={{ opacity: 0.45, fontSize: 8, position: "absolute", top: 2, left: 4 }}>
                  {k.index}
                </span>
                {act && (
                  <span
                    style={{
                      position: "absolute",
                      top: 3,
                      right: hasOverride ? 11 : 4,
                      width: 6,
                      height: 6,
                      borderRadius: 99,
                      background: act.accent || "#fa6831",
                    }}
                  />
                )}
                {hasOverride && (
                  <span
                    style={{
                      position: "absolute",
                      top: 3,
                      right: 4,
                      width: 6,
                      height: 6,
                      borderRadius: 99,
                      background: "#4de1ff",
                    }}
                  />
                )}
                <span style={{ maxWidth: "95%", textOverflow: "ellipsis", overflow: "hidden", whiteSpace: "nowrap" }}>
                  {dispLabel || "—"}
                </span>
              </button>
            );
          })}
        </div>
      </div>

      {/* Selected Key Inspector & Custom Override Editor */}
      <div className="card" style={{ padding: 14, display: "flex", flexDirection: "column", gap: 12 }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ margin: 0 }}>Key {selectedKey} Inspector</h3>
          <span className="pill" style={{ fontSize: 11 }}>
            Layer {selectedResolved?.source_layer ?? activeLayer}
          </span>
        </div>

        {/* Binding & Owner Info */}
        <div style={{ padding: 10, borderRadius: 8, background: "rgba(255,255,255,0.04)", fontSize: 12 }}>
          <div style={{ marginBottom: 6 }}>
            <strong>QMK Binding: </strong>
            {selectedAction ? (
              <span style={{ color: selectedAction.accent || "#fa6831" }}>
                {selectedAction.label || selectedAction.action_id} ({selectedAction.module_id ?? "built-in"} ·{" "}
                {selectedAction.mode ?? "tap"} · {formatHexKeycode(selectedResolved?.keycode ?? 0)})
              </span>
            ) : selectedResolved ? (
              <span>{formatHexKeycode(selectedResolved.keycode)}</span>
            ) : (
              <span className="muted">Waiting for keyboard…</span>
            )}
          </div>
          <div>
            <strong>Active Visual Stack:</strong>
            {selectedClaims.length === 0 ? (
              <div className="muted" style={{ marginTop: 4 }}>No active provider paints this key (transparent background).</div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 4 }}>
                {selectedClaims.map((c, idx) => (
                  <div
                    key={`${c.topic}-${c.priority}-${idx}`}
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      padding: "3px 6px",
                      borderRadius: 5,
                      background: idx === 0 ? "rgba(77, 225, 255, 0.14)" : "rgba(255,255,255,0.03)",
                      fontSize: 11,
                    }}
                  >
                    <span>
                      {idx === 0 ? "★ " : ""}
                      <strong>{c.topic}</strong> (prio {c.priority})
                    </span>
                    <span className="muted">
                      {c.rule.image
                        ? `PNG: ${c.rule.image.split("/").pop()}`
                        : c.rule.icon
                          ? `Icon: ${c.rule.icon}${c.rule.label ? ` · ${c.rule.label}` : ""}`
                          : c.rule.label || "(blank)"}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Override Scope */}
        <label className="field">
          <span>Override Scope</span>
          <select value={scope} onChange={(e) => loadDraftForScope(e.target.value as "action" | "key_layer" | "key_all")}>
            {selectedAction && (
              <option value="action">
                Action: {selectedAction.label || selectedAction.action_id} (follows action onto any key/layer)
              </option>
            )}
            <option value="key_layer">Physical Key {selectedKey} on Layer {activeLayer}</option>
            <option value="key_all">Physical Key {selectedKey} on All Layers</option>
          </select>
        </label>

        {/* Override Fields */}
        <label className="field">
          <span>Custom Label (leave blank to keep default or show icon-only)</span>
          <input
            type="text"
            placeholder={selectedWinner?.rule.label ?? "e.g. Cam"}
            value={draft.label ?? ""}
            onChange={(e) => setDraft((p) => ({ ...p, label: e.target.value }))}
          />
        </label>

        <div className="row-2">
          <label className="field">
            <span>Built-in Vector Icon</span>
            <select
              value={draft.icon ?? ""}
              onChange={(e) => setDraft((p) => ({ ...p, icon: e.target.value, image: e.target.value ? "" : p.image }))}
            >
              {BUILTIN_ICONS.map((ic) => (
                <option key={ic || "none"} value={ic}>
                  {ic ? ic : "None / Default"}
                </option>
              ))}
            </select>
          </label>
          <div className="field">
            <span>Custom PNG Image</span>
            <div style={{ display: "flex", gap: 6 }}>
              <button type="button" className="btn-secondary" style={{ flex: 1 }} onClick={() => void handlePickImage()}>
                {draft.image ? draft.image : "Choose PNG…"}
              </button>
              {draft.image && (
                <button type="button" className="btn-secondary" onClick={() => setDraft((p) => ({ ...p, image: "" }))}>
                  ✕
                </button>
              )}
            </div>
          </div>
        </div>

        <div className="row-3">
          <label className="field">
            <span>Text / Icon Color</span>
            <input
              type="text"
              placeholder="#ffffff"
              value={draft.color ?? ""}
              onChange={(e) => setDraft((p) => ({ ...p, color: e.target.value }))}
            />
          </label>
          <label className="field">
            <span>Keycap Tint</span>
            <input
              type="text"
              placeholder="#1a2436"
              value={draft.bg ?? ""}
              onChange={(e) => setDraft((p) => ({ ...p, bg: e.target.value }))}
            />
          </label>
          <label className="field">
            <span>Border Color</span>
            <input
              type="text"
              placeholder="#4de1ff"
              value={draft.border ?? ""}
              onChange={(e) => setDraft((p) => ({ ...p, border: e.target.value }))}
            />
          </label>
        </div>

        <label style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 12 }}>
          <input
            type="checkbox"
            checked={Boolean(draft.hide)}
            onChange={(e) => setDraft((p) => ({ ...p, hide: e.target.checked }))}
          />
          Hide legend/icon completely on this key
        </label>

        <div style={{ display: "flex", gap: 8 }}>
          <button type="button" className="btn-primary" style={{ flex: 1 }} disabled={saving} onClick={() => void handleSave()}>
            {saving ? "Saving…" : "Apply Override"}
          </button>
          <button type="button" className="btn-secondary" disabled={saving} onClick={() => void handleReset()}>
            Reset
          </button>
        </div>
        {statusMsg && <div style={{ fontSize: 12, color: "#7ae7ff" }}>{statusMsg}</div>}
      </div>
    </div>
  );
};
