import type React from "react";
import { useCallback, useEffect, useState } from "react";
import { type ContextRule, type ContextTopBarSlot, type V2ModuleRecord, v2Client } from "../api/v2Client";

interface AppContextViewProps {
  modules: V2ModuleRecord[];
}

const LAYER_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "", label: "Keep Base QMK Layer" },
  { value: "1", label: "Layer 1 (Fn)" },
  { value: "2", label: "Layer 2 (Gaming)" },
  { value: "3", label: "Layer 3" },
  { value: "4", label: "Layer 4" },
  { value: "5", label: "Layer 5" },
  { value: "6", label: "Layer 6" },
  { value: "7", label: "Layer 7" },
];

export const AppContextView: React.FC<AppContextViewProps> = ({ modules }) => {
  const [rules, setRules] = useState<ContextRule[]>([]);
  const [activeCtx, setActiveCtx] = useState<{
    active: boolean;
    rule_id?: string;
    rule_name?: string;
    app_name?: string;
    qmk_layer?: number | null;
  }>({ active: false });
  const [currentApp, setCurrentApp] = useState<string>("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const slotWidgets = modules.filter((m) => m.manifest.slot_capable);

  const loadRules = useCallback(async () => {
    try {
      const res = await v2Client.getContextRules();
      setRules(res.rules ?? []);
      if (res.active_context) setActiveCtx(res.active_context);
      setCurrentApp(res.current_app?.process_name || res.current_app?.app_id || "");
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void loadRules();
    const id = window.setInterval(() => void loadRules(), 2500);
    return () => window.clearInterval(id);
  }, [loadRules]);

  const saveAll = async (next: ContextRule[], note?: string) => {
    setRules(next);
    try {
      await v2Client.putContextRules(next);
      if (note) setStatusMsg(note);
      await loadRules();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleAddRule = () => {
    const id = `ctx_${Date.now()}`;
    const nextRule: ContextRule = {
      id,
      name: currentApp ? `${currentApp} Setup` : "VS Code / Coding",
      enabled: true,
      match_apps: currentApp ? [currentApp] : ["Code", "Cursor"],
      qmk_layer: null,
      top_bar_slots: null,
    };
    const next = [...rules, nextRule];
    setSelectedId(id);
    void saveAll(next, `Added context rule "${nextRule.name}".`);
  };

  const updateRule = (id: string, patch: Partial<ContextRule>) => {
    const next = rules.map((r) => (r.id === id ? { ...r, ...patch } : r));
    void saveAll(next, "Saved context rule.");
  };

  const deleteRule = (id: string) => {
    const next = rules.filter((r) => r.id !== id);
    if (selectedId === id) setSelectedId(null);
    void saveAll(next, "Deleted context rule.");
  };

  const selected = rules.find((r) => r.id === selectedId) ?? rules[0] ?? null;

  const setBayWidget = (rule: ContextRule, bayIdx: number, modId: string) => {
    const existing = (rule.top_bar_slots ?? []).filter((s) => Math.round(s.pos) !== bayIdx);
    const nextSlots: ContextTopBarSlot[] = modId
      ? [...existing, { module_id: modId, pos: bayIdx, span: 1.0 }].sort((a, b) => a.pos - b.pos)
      : existing;
    updateRule(rule.id, { top_bar_slots: nextSlots });
  };

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>App Context</h1>
          <span className="kg-pill-subtle">Touch Bar &amp; Per-App Keybindings</span>
          <span className={`kg-status-badge ${activeCtx.active ? "is-ok" : "is-warn"}`}>
            {activeCtx.active
              ? `Active: ${activeCtx.rule_name}${activeCtx.qmk_layer != null ? ` (L${activeCtx.qmk_layer})` : ""}`
              : currentApp
                ? `Focused: ${currentApp} (Default)`
                : "Default Context"}
          </span>
        </div>
        <div className="kg-header-actions">
          <button type="button" className="kg-btn-primary" onClick={handleAddRule}>
            + New App Rule
          </button>
        </div>
      </div>

      <p className="kg-page-desc">
        Switch your Top Bar widgets (Apple Touch Bar style) and active QMK keybinding layer automatically in &lt;1 ms
        whenever you switch applications — zero flash writes, zero typing stutter.
      </p>

      {rules.length === 0 ? (
        <div className="kg-calm-card">
          <div className="kg-calm-row">
            <div>
              <div className="kg-row-title">No per-app rules yet</div>
              <div className="kg-muted">
                Create a rule for VS Code, Figma, Blender, Spotify, or any game to automatically switch Top Bar widgets
                and QMK keymap layers when that app is focused.
              </div>
            </div>
            <button type="button" className="kg-btn-primary" onClick={handleAddRule}>
              + Create First Rule
            </button>
          </div>
        </div>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "260px minmax(0, 1fr)", gap: 16, alignItems: "start" }}>
          {/* Rules List */}
          <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 6, padding: 10 }}>
            {rules.map((r) => {
              const isSel = selected?.id === r.id;
              const isLive = activeCtx.active && activeCtx.rule_id === r.id;
              return (
                <button
                  key={r.id}
                  type="button"
                  className={`kg-nav-item ${isSel ? "is-active" : ""}`}
                  onClick={() => setSelectedId(r.id)}
                >
                  <span style={{ flex: 1 }}>
                    <div style={{ fontWeight: 600 }}>{r.name}</div>
                    <div className="kg-muted" style={{ fontSize: 11 }}>
                      {r.match_apps.join(", ") || "No apps"}
                      {r.qmk_layer != null ? ` · L${r.qmk_layer}` : ""}
                      {r.top_bar_slots ? " · Custom Bar" : ""}
                    </div>
                  </span>
                  {isLive && <span className="kg-conn-dot is-on" title="Currently active" />}
                </button>
              );
            })}
          </div>

          {/* Selected Rule Editor */}
          {selected && (
            <div className="kg-calm-card kg-settings-stack">
              <div className="kg-calm-row">
                <div className="kg-row-title">{selected.name}</div>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 12 }}>
                    <input
                      type="checkbox"
                      checked={selected.enabled}
                      onChange={(e) => updateRule(selected.id, { enabled: e.target.checked })}
                    />
                    Enabled
                  </label>
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    onClick={() => deleteRule(selected.id)}
                  >
                    Delete
                  </button>
                </div>
              </div>

              <div className="kg-setting-row">
                <div className="kg-setting-meta">
                  <div className="kg-setting-label">Rule Name</div>
                </div>
                <div className="kg-setting-ctrl">
                  <input
                    type="text"
                    value={selected.name}
                    onChange={(e) => updateRule(selected.id, { name: e.target.value })}
                  />
                </div>
              </div>

              <div className="kg-setting-row">
                <div className="kg-setting-meta">
                  <div className="kg-setting-label">Match Applications (comma-separated)</div>
                  <div className="kg-muted">Matches process name, bundle ID, or window title</div>
                </div>
                <div className="kg-setting-ctrl" style={{ flexDirection: "column", alignItems: "flex-end" }}>
                  <input
                    type="text"
                    style={{ width: "100%" }}
                    value={selected.match_apps.join(", ")}
                    onChange={(e) =>
                      updateRule(selected.id, {
                        match_apps: e.target.value
                          .split(",")
                          .map((s) => s.trim())
                          .filter(Boolean),
                      })
                    }
                  />
                  {currentApp && (
                    <button
                      type="button"
                      className="kg-btn-ghost"
                      onClick={() => {
                        if (!selected.match_apps.includes(currentApp)) {
                          updateRule(selected.id, { match_apps: [...selected.match_apps, currentApp] });
                        }
                      }}
                    >
                      + Add Focused App ({currentApp})
                    </button>
                  )}
                </div>
              </div>

              <div className="kg-setting-row">
                <div className="kg-setting-meta">
                  <div className="kg-setting-label">Per-App QMK Keybindings Layer</div>
                  <div className="kg-muted">
                    Instantly switches RP2040 hardware keybindings &amp; keycaps in RAM (`0x75`)
                  </div>
                </div>
                <div className="kg-setting-ctrl">
                  <select
                    value={selected.qmk_layer == null ? "" : String(selected.qmk_layer)}
                    onChange={(e) =>
                      updateRule(selected.id, {
                        qmk_layer: e.target.value === "" ? null : Number(e.target.value),
                      })
                    }
                  >
                    {LAYER_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="kg-setting-row" style={{ flexDirection: "column", alignItems: "stretch" }}>
                <div className="kg-calm-row">
                  <div>
                    <div className="kg-setting-label">Per-App Top Bar (“Touch Bar” Override)</div>
                    <div className="kg-muted">
                      Show a different set of Top Bar widgets whenever this app is focused
                    </div>
                  </div>
                  <input
                    type="checkbox"
                    checked={selected.top_bar_slots != null}
                    onChange={(e) =>
                      updateRule(selected.id, {
                        top_bar_slots: e.target.checked
                          ? [
                              { module_id: "widget-clock", pos: 0, span: 1 },
                              { module_id: "widget-system-load", pos: 1, span: 1 },
                            ]
                          : null,
                      })
                    }
                  />
                </div>

                {selected.top_bar_slots != null && (
                  <div
                    style={{
                      display: "grid",
                      gridTemplateColumns: "repeat(4, minmax(0, 1fr))",
                      gap: 10,
                      marginTop: 10,
                    }}
                  >
                    {[0, 1, 2, 3].map((bayIdx) => {
                      const cur = selected.top_bar_slots?.find((s) => Math.round(s.pos) === bayIdx);
                      return (
                        <label key={bayIdx} className="kg-field-inline" style={{ minWidth: 0 }}>
                          <span>Bay {bayIdx + 1}</span>
                          <select
                            value={cur?.module_id ?? ""}
                            onChange={(e) => setBayWidget(selected, bayIdx, e.target.value)}
                          >
                            <option value="">Empty</option>
                            {slotWidgets.map((w) => (
                              <option key={w.id} value={w.id}>
                                {w.manifest.name}
                              </option>
                            ))}
                          </select>
                        </label>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {statusMsg && <div className="kg-muted">{statusMsg}</div>}
    </div>
  );
};
