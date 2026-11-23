import type React from "react";
import { useEffect, useState } from "react";
import { type V2ModuleRecord, v2Client } from "../api/v2Client";
import { type ActionSlotInfo, decodeQmkKeycode } from "../api/vialProtocol";
import { KeyboardSurface, type SurfaceKeyGeom } from "./KeyboardSurface";

interface ResolvedKey {
  keycode: number;
  source_layer: number;
}

interface ProviderClaim {
  topic: string;
  priority: number;
  label?: string;
  icon?: string;
  image?: string;
}

interface HomeViewProps {
  keys: SurfaceKeyGeom[];
  modules: V2ModuleRecord[];
  onOpenModule: (moduleId: string, focusKey?: number) => void;
  onOpenStore: () => void;
  previewTick: number;
}

export const HomeView: React.FC<HomeViewProps> = ({
  keys,
  modules,
  onOpenModule,
  onOpenStore,
  previewTick,
}) => {
  const [selectedKey, setSelectedKey] = useState<number | null>(null);
  const [topBarSelected, setTopBarSelected] = useState(false);
  const [resolved, setResolved] = useState<ResolvedKey[]>([]);
  const [activeLayer, setActiveLayer] = useState(0);
  const [actionsBySlot, setActionsBySlot] = useState<Record<number, ActionSlotInfo>>({});
  const [claimsByKey, setClaimsByKey] = useState<Record<number, ProviderClaim[]>>({});

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const [rkRes, actRes, provRes] = await Promise.all([
          v2Client.getBusRetained("keyboard/resolved_keys"),
          v2Client.getBusRetained("system/actions"),
          v2Client.getBusRetained("keys/providers/#"),
        ]);
        if (!alive) return;
        const rk = rkRes.messages[0]?.payload as { active_layer?: number; keys?: ResolvedKey[] } | undefined;
        if (typeof rk?.active_layer === "number") setActiveLayer(rk.active_layer);
        if (Array.isArray(rk?.keys)) setResolved(rk.keys);

        const sa = actRes.messages[0]?.payload as { slots?: Record<string, ActionSlotInfo> } | undefined;
        const amap: Record<number, ActionSlotInfo> = {};
        if (sa?.slots) {
          for (const [k, v] of Object.entries(sa.slots)) {
            amap[Number(k)] = { ...v, slot: Number(k) };
          }
        }
        setActionsBySlot(amap);

        const cmap: Record<number, ProviderClaim[]> = {};
        for (const m of provRes.messages) {
          const p = m.payload as { priority?: number; rules?: Array<Record<string, unknown>> } | null;
          if (!p || !Array.isArray(p.rules)) continue;
          const baseP = typeof p.priority === "number" ? p.priority : 10;
          for (const r of p.rules) {
            if (typeof r.mods === "number" && r.mods > 0) continue;
            if (typeof r.key !== "number") continue;
            const prio = typeof r.priority === "number" ? r.priority : baseP;
            const list = cmap[r.key] ?? [];
            list.push({
              topic: m.topic.replace("keys/providers/", ""),
              priority: prio,
              label: typeof r.label === "string" ? r.label : undefined,
              icon: typeof r.icon === "string" ? r.icon : undefined,
              image: typeof r.image === "string" ? r.image : undefined,
            });
            cmap[r.key] = list;
          }
        }
        for (const k of Object.keys(cmap)) {
          cmap[Number(k)].sort((a, b) => b.priority - a.priority);
        }
        setClaimsByKey(cmap);
      } catch {
        // Offline or preview-only state
      }
    };
    void load();
    const id = window.setInterval(() => void load(), 3000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);

  const enabledModules = modules.filter((m) => m.state.enabled);
  const topBarWidgets = enabledModules
    .filter((m) => m.manifest.slot_capable && (m.state.slot_pos != null || m.state.slot != null))
    .sort((a, b) => (a.state.slot_pos ?? a.state.slot ?? 0) - (b.state.slot_pos ?? b.state.slot ?? 0));
  const bgModules = enabledModules.filter((m) => m.manifest.category === "background");

  const selResolved = selectedKey != null ? resolved[selectedKey] : undefined;
  const selDecoded = selResolved ? decodeQmkKeycode(selResolved.keycode, actionsBySlot) : null;
  const selClaims = selectedKey != null ? (claimsByKey[selectedKey] ?? []) : [];
  const selWin = selClaims[0];

  const handleSelectKey = (k: number) => {
    setTopBarSelected(false);
    setSelectedKey((prev) => (prev === k ? null : k));
  };

  const handleSelectTopBar = () => {
    setSelectedKey(null);
    setTopBarSelected((prev) => !prev);
  };

  return (
    <div className="kg-home">
      <div className="kg-home-stage">
        <KeyboardSurface
          keys={keys}
          selectedKey={selectedKey}
          topBarSelected={topBarSelected}
          onSelectKey={handleSelectKey}
          onSelectTopBar={handleSelectTopBar}
          showLivePreview
          previewTick={previewTick}
        />
      </div>

      {/* Single calm, contextual inspector card below the keyboard (5a aesthetic) */}
      <div className="kg-home-inspector">
        {enabledModules.length === 0 ? (
          <div className="kg-calm-card kg-empty-shell">
            <div className="kg-calm-row">
              <div>
                <strong>No modules enabled yet</strong>
                <div className="kg-muted">
                  KeyGlass is a thin modular shell — enable a keymap, visuals, or widgets to make the keyboard yours.
                </div>
              </div>
              <button type="button" className="kg-btn-primary" onClick={onOpenStore}>
                + Add Module
              </button>
            </div>
          </div>
        ) : selectedKey != null ? (
          <div className="kg-calm-card">
            <div className="kg-calm-row">
              <div>
                <div className="kg-row-title">QMK Keymap</div>
                <div className="kg-muted">
                  Key {selectedKey} · Layer {selResolved?.source_layer ?? activeLayer}
                  {selDecoded
                    ? ` — Bound to: ${selDecoded.label}${selDecoded.sub ? ` (${selDecoded.sub})` : ""}`
                    : " — Click Edit to inspect or remap in QMK"}
                </div>
              </div>
              <button
                type="button"
                className="kg-btn-ghost"
                onClick={() => onOpenModule("keys-keymap", selectedKey)}
              >
                Edit
              </button>
            </div>
            <div className="kg-calm-divider" />
            <div className="kg-calm-row">
              <div>
                <div className="kg-row-title">Keycap Visuals</div>
                <div className="kg-muted">
                  {selWin
                    ? selWin.image
                      ? `Icon: ${selWin.image.split("/").pop()} (from ${selWin.topic})`
                      : selWin.icon
                        ? `Symbol: ${selWin.icon}${selWin.label ? ` · ${selWin.label}` : ""} (from ${selWin.topic})`
                        : `Label: ${selWin.label || "Blank"} (from ${selWin.topic})`
                    : "Default keycap appearance — click Override to set a custom label, icon, or PNG"}
                </div>
              </div>
              <button
                type="button"
                className="kg-btn-ghost"
                onClick={() => onOpenModule("keys-custom", selectedKey)}
              >
                Override
              </button>
            </div>
          </div>
        ) : topBarSelected ? (
          <div className="kg-calm-card">
            <div className="kg-calm-row">
              <div>
                <div className="kg-row-title">Top Bar Dock</div>
                <div className="kg-muted">
                  {topBarWidgets.length > 0
                    ? `Active widgets: ${topBarWidgets.map((w) => w.manifest.name).join(" · ")}`
                    : "No widgets placed in the top display strip"}
                </div>
              </div>
              <button
                type="button"
                className="kg-btn-ghost"
                onClick={() => onOpenModule("dock-top-bar")}
              >
                Arrange
              </button>
            </div>
          </div>
        ) : (
          <div className="kg-calm-hint">
            <span>Click any key or the top display bar to inspect which modules control it.</span>
            {bgModules.length > 0 && (
              <span className="kg-muted">
                {" · "}
                Background: {bgModules.map((m) => m.manifest.name).join(", ")}
              </span>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
