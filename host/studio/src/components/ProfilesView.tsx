import type React from "react";
import { useCallback, useEffect, useState } from "react";
import {
  type ActiveProfileState,
  type ContextTopBarSlot,
  type HostIdentity,
  type ProfileEntry,
  type V2ModuleRecord,
  v2Client,
} from "../api/v2Client";

interface ProfilesViewProps {
  modules: V2ModuleRecord[];
}

const LAYER_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "", label: "Fall through (inherit from parent / Base)" },
  { value: "0", label: "Layer 0 (Base)" },
  { value: "1", label: "Layer 1 (Fn)" },
  { value: "2", label: "Layer 2 (Gaming)" },
  { value: "3", label: "Layer 3" },
  { value: "4", label: "Layer 4" },
  { value: "5", label: "Layer 5" },
  { value: "6", label: "Layer 6" },
  { value: "7", label: "Layer 7" },
];

export const ProfilesView: React.FC<ProfilesViewProps> = ({ modules }) => {
  const [profiles, setProfiles] = useState<ProfileEntry[]>([]);
  const [manualId, setManualId] = useState<string | null>(null);
  const [active, setActive] = useState<ActiveProfileState | null>(null);
  const [host, setHost] = useState<HostIdentity | null>(null);
  const [hostNameDraft, setHostNameDraft] = useState("");
  const [currentApp, setCurrentApp] = useState("");
  const [selectedId, setSelectedId] = useState<string>("default");
  const [overrideModId, setOverrideModId] = useState<string>("bg-media");
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const slotWidgets = modules.filter((m) => m.manifest.slot_capable);
  const configurableMods = modules.filter((m) => m.manifest.settings.length > 0);

  const loadAll = useCallback(async () => {
    try {
      const res = await v2Client.getProfiles();
      setProfiles(res.profiles ?? []);
      setManualId(res.manual_profile_id ?? null);
      setActive(res.active ?? null);
      if (res.host) {
        setHost(res.host);
        setHostNameDraft((prev) => (prev ? prev : res.host.host_name));
      }
      setCurrentApp(res.current_app?.process_name || res.current_app?.app_id || "");
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void loadAll();
    const id = window.setInterval(() => void loadAll(), 2500);
    return () => window.clearInterval(id);
  }, [loadAll]);

  const persist = async (nextManual: string | null, nextProfiles: ProfileEntry[], note?: string) => {
    setManualId(nextManual);
    setProfiles(nextProfiles);
    try {
      await v2Client.putProfiles(nextManual, nextProfiles);
      if (note) setStatusMsg(note);
      await loadAll();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleSaveHostName = async () => {
    if (!host || !hostNameDraft.trim()) return;
    try {
      const next = { ...host, host_name: hostNameDraft.trim() };
      await v2Client.putHostIdentity(next);
      setHost(next);
      setStatusMsg(`Computer name saved as "${next.host_name}" (${next.os}).`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCreateProfile = () => {
    const slug = `profile_${Date.now().toString(36)}`;
    const nextP: ProfileEntry = {
      id: slug,
      name: currentApp ? `${currentApp}` : "Coding",
      parent: "default",
      enabled: true,
      match_apps: currentApp ? [currentApp] : ["Code", "Cursor"],
      match_hosts: [],
      match_os: [],
      tags: [],
      qmk_layer: null,
      top_bar_slots: null,
      module_overrides: {},
    };
    setSelectedId(slug);
    void persist(manualId, [...profiles, nextP], `Created profile "${nextP.name}" inheriting from Default.`);
  };

  const updateProfile = (id: string, patch: Partial<ProfileEntry>) => {
    const next = profiles.map((p) => (p.id === id ? { ...p, ...patch } : p));
    void persist(manualId, next, "Saved profile.");
  };

  const deleteProfile = (id: string) => {
    if (id === "default") return;
    const next = profiles
      .filter((p) => p.id !== id)
      .map((p) => (p.parent === id ? { ...p, parent: "default" } : p));
    const nextManual = manualId === id ? null : manualId;
    setSelectedId("default");
    void persist(nextManual, next, "Deleted profile.");
  };

  const handleSelectManual = async (pid: string | null) => {
    try {
      await v2Client.selectProfile(pid);
      setManualId(pid);
      await loadAll();
      setStatusMsg(pid ? `Pinned profile "${pid}" manually.` : "Returned to automatic profile matching.");
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const selected = profiles.find((p) => p.id === selectedId) ?? profiles[0] ?? null;

  const setBayWidget = (p: ProfileEntry, bayIdx: number, modId: string) => {
    const existing = (p.top_bar_slots ?? []).filter((s) => Math.round(s.pos) !== bayIdx);
    const nextSlots: ContextTopBarSlot[] = modId
      ? [...existing, { module_id: modId, pos: bayIdx, span: 1.0 }].sort((a, b) => a.pos - b.pos)
      : existing;
    updateProfile(p.id, { top_bar_slots: nextSlots });
  };

  const setModuleOverride = (p: ProfileEntry, modId: string, key: string, value: unknown) => {
    const nextAll = { ...(p.module_overrides ?? {}) };
    const nextMod = { ...(nextAll[modId] ?? {}) };
    if (value === undefined || value === "") {
      delete nextMod[key];
    } else {
      nextMod[key] = value;
    }
    if (Object.keys(nextMod).length === 0) {
      delete nextAll[modId];
    } else {
      nextAll[modId] = nextMod;
    }
    updateProfile(p.id, { module_overrides: nextAll });
  };

  const targetOverrideMod =
    configurableMods.find((m) => m.id === overrideModId) ?? configurableMods[0] ?? null;

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>Profiles</h1>
          <span className="kg-pill-subtle">Module · Inheritance &amp; Multi-Computer</span>
          {active && (
            <span className="kg-status-badge is-ok">
              Active: {active.profile_name} ({active.chain.join(" → ")})
            </span>
          )}
        </div>
        <div className="kg-header-actions">
          {manualId && (
            <button type="button" className="kg-btn-ghost" onClick={() => void handleSelectManual(null)}>
              Return to Auto
            </button>
          )}
          <button type="button" className="kg-btn-primary" onClick={handleCreateProfile}>
            + New Profile
          </button>
        </div>
      </div>

      {/* Host / Multi-Computer Identity Bar (lives ABOVE Profiles!) */}
      {host && (
        <div className="kg-calm-card kg-calm-row" style={{ flexWrap: "wrap" }}>
          <div>
            <div className="kg-row-title">This Computer (Multi-Computer Identity)</div>
            <div className="kg-muted">
              ID: <code>{host.host_id}</code> · OS: <strong>{host.os}</strong> — Profiles can match specific
              computers or OSes, and each computer’s profiles are synced separately to the keyboard.
            </div>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <input
              type="text"
              value={hostNameDraft}
              onChange={(e) => setHostNameDraft(e.target.value)}
              placeholder="e.g. Work MacBook / Gaming PC"
              style={{ width: 200 }}
            />
            <button type="button" className="kg-btn-ghost" onClick={() => void handleSaveHostName()}>
              Rename Host
            </button>
          </div>
        </div>
      )}

      <div style={{ display: "grid", gridTemplateColumns: "256px minmax(0, 1fr)", gap: 16, alignItems: "start" }}>
        {/* Profiles List with Inheritance & Pin Controls */}
        <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 6, padding: 10 }}>
          {profiles.map((p) => {
            const isSel = selected?.id === p.id;
            const isActive = active?.profile_id === p.id;
            const isPinned = manualId === p.id;
            const parentName = p.parent ? profiles.find((x) => x.id === p.parent)?.name ?? p.parent : null;
            return (
              <button
                key={p.id}
                type="button"
                className={`kg-nav-item ${isSel ? "is-active" : ""}`}
                onClick={() => setSelectedId(p.id)}
              >
                <span style={{ flex: 1 }}>
                  <div style={{ fontWeight: 600 }}>
                    {p.name} {isPinned ? "📌" : ""}
                  </div>
                  <div className="kg-muted" style={{ fontSize: 11 }}>
                    {parentName ? `↳ ${parentName}` : "Root"}
                    {p.qmk_layer != null ? ` · L${p.qmk_layer}` : ""}
                    {p.top_bar_slots ? " · Bar" : ""}
                    {Object.keys(p.module_overrides ?? {}).length > 0
                      ? ` · ${Object.keys(p.module_overrides).length} mod`
                      : ""}
                  </div>
                </span>
                {isActive && <span className="kg-conn-dot is-on" title="Currently active profile" />}
              </button>
            );
          })}
        </div>

        {/* Selected Profile Editor */}
        {selected && (
          <div className="kg-calm-card kg-settings-stack">
            <div className="kg-calm-row" style={{ flexWrap: "wrap" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <div className="kg-row-title">{selected.name}</div>
                <span className="kg-pill-subtle">id: {selected.id}</span>
              </div>
              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <button
                  type="button"
                  className={manualId === selected.id ? "kg-btn-primary" : "kg-btn-ghost"}
                  onClick={() => void handleSelectManual(manualId === selected.id ? null : selected.id)}
                >
                  {manualId === selected.id ? "Pinned Active (Unpin)" : "Pin Active Now"}
                </button>
                {selected.id !== "default" && (
                  <button type="button" className="kg-btn-ghost" onClick={() => deleteProfile(selected.id)}>
                    Delete
                  </button>
                )}
              </div>
            </div>

            <div className="kg-setting-row">
              <div className="kg-setting-meta">
                <div className="kg-setting-label">Profile Name &amp; Fall-Through Parent</div>
                <div className="kg-muted">Unset settings cleanly fall through to the parent profile</div>
              </div>
              <div className="kg-setting-ctrl">
                <input
                  type="text"
                  value={selected.name}
                  onChange={(e) => updateProfile(selected.id, { name: e.target.value })}
                  style={{ width: 150 }}
                />
                {selected.id !== "default" && (
                  <select
                    value={selected.parent ?? "default"}
                    onChange={(e) => updateProfile(selected.id, { parent: e.target.value || null })}
                  >
                    {profiles
                      .filter((p) => p.id !== selected.id)
                      .map((p) => (
                        <option key={p.id} value={p.id}>
                          Inherits: {p.name}
                        </option>
                      ))}
                  </select>
                )}
              </div>
            </div>

            {selected.id !== "default" && (
              <>
                <div className="kg-setting-row">
                  <div className="kg-setting-meta">
                    <div className="kg-setting-label">Auto-Activate on Applications</div>
                    <div className="kg-muted">Comma-separated app names, bundle IDs, or window titles</div>
                  </div>
                  <div className="kg-setting-ctrl" style={{ flexDirection: "column", alignItems: "flex-end" }}>
                    <input
                      type="text"
                      style={{ width: "100%" }}
                      placeholder="e.g. Code, Cursor, Figma"
                      value={selected.match_apps.join(", ")}
                      onChange={(e) =>
                        updateProfile(selected.id, {
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
                            updateProfile(selected.id, { match_apps: [...selected.match_apps, currentApp] });
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
                    <div className="kg-setting-label">Computer / OS Scope (Multi-Computer)</div>
                    <div className="kg-muted">Leave blank to match all computers &amp; OSes</div>
                  </div>
                  <div className="kg-setting-ctrl">
                    <select
                      value={selected.match_os[0] ?? ""}
                      onChange={(e) =>
                        updateProfile(selected.id, { match_os: e.target.value ? [e.target.value] : [] })
                      }
                    >
                      <option value="">Any OS</option>
                      <option value="macos">macOS only</option>
                      <option value="windows">Windows only</option>
                      <option value="linux">Linux only</option>
                    </select>
                    {host && (
                      <button
                        type="button"
                        className={selected.match_hosts.includes(host.host_id) ? "kg-btn-primary" : "kg-btn-ghost"}
                        onClick={() =>
                          updateProfile(selected.id, {
                            match_hosts: selected.match_hosts.includes(host.host_id) ? [] : [host.host_id],
                          })
                        }
                      >
                        {selected.match_hosts.includes(host.host_id) ? `Only ${host.host_name}` : "This Computer Only"}
                      </button>
                    )}
                  </div>
                </div>
              </>
            )}

            <div className="kg-setting-row">
              <div className="kg-setting-meta">
                <div className="kg-setting-label">QMK Keybindings Layer</div>
                <div className="kg-muted">Switches hardware keybindings &amp; keycaps in RAM (`0x75`)</div>
              </div>
              <div className="kg-setting-ctrl">
                <select
                  value={selected.qmk_layer == null ? "" : String(selected.qmk_layer)}
                  onChange={(e) =>
                    updateProfile(selected.id, {
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

            {/* Per-Profile Top Bar ("Touch Bar") Override */}
            <div className="kg-setting-row" style={{ flexDirection: "column", alignItems: "stretch" }}>
              <div className="kg-calm-row">
                <div>
                  <div className="kg-setting-label">Top Bar (“Touch Bar”) Layout Override</div>
                  <div className="kg-muted">Leave unchecked to inherit from parent / default Top Bar</div>
                </div>
                <input
                  type="checkbox"
                  checked={selected.top_bar_slots != null}
                  onChange={(e) =>
                    updateProfile(selected.id, {
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
                <div style={{ display: "grid", gridTemplateColumns: "repeat(4, minmax(0, 1fr))", gap: 10, marginTop: 10 }}>
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

            {/* Per-Profile Module Setting Overrides (any module inherits & overrides cleanly!) */}
            <div className="kg-setting-row" style={{ flexDirection: "column", alignItems: "stretch" }}>
              <div className="kg-calm-row" style={{ marginBottom: 8 }}>
                <div>
                  <div className="kg-setting-label">Per-Profile Module Setting Overrides</div>
                  <div className="kg-muted">
                    Override any setting of any module just for this profile (falls through to parent/global otherwise!)
                  </div>
                </div>
                <select value={overrideModId} onChange={(e) => setOverrideModId(e.target.value)}>
                  {configurableMods.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.manifest.name}
                      {selected.module_overrides?.[m.id] ? " ★" : ""}
                    </option>
                  ))}
                </select>
              </div>
              {targetOverrideMod && (
                <div style={{ display: "flex", flexDirection: "column", gap: 8, padding: 10, borderRadius: 10, background: "rgba(255,255,255,0.025)" }}>
                  {targetOverrideMod.manifest.settings.slice(0, 6).map((field) => {
                    const ovVal = selected.module_overrides?.[targetOverrideMod.id]?.[field.key];
                    const hasOv = ovVal !== undefined;
                    const displayVal = hasOv ? ovVal : (targetOverrideMod.state.config?.[field.key] ?? field.default ?? "");
                    return (
                      <div key={field.key} className="kg-calm-row" style={{ fontSize: 12 }}>
                        <span>
                          {field.label}{" "}
                          {hasOv ? (
                            <span className="kg-pill-accent">Overridden</span>
                          ) : (
                            <span className="kg-muted">(inherited)</span>
                          )}
                        </span>
                        <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                          {field.kind === "boolean" ? (
                            <input
                              type="checkbox"
                              checked={Boolean(displayVal)}
                              onChange={(e) =>
                                setModuleOverride(selected, targetOverrideMod.id, field.key, e.target.checked)
                              }
                            />
                          ) : field.kind === "select" ? (
                            <select
                              value={String(displayVal)}
                              onChange={(e) =>
                                setModuleOverride(selected, targetOverrideMod.id, field.key, e.target.value)
                              }
                            >
                              {(field.options ?? []).map((opt) => (
                                <option key={opt} value={opt}>
                                  {opt}
                                </option>
                              ))}
                            </select>
                          ) : field.kind === "number" ? (
                            <input
                              type="number"
                              step={(field.max ?? 100) - (field.min ?? 0) <= 5 ? 0.05 : 1}
                              value={Number(displayVal) || 0}
                              style={{ width: 90 }}
                              onChange={(e) =>
                                setModuleOverride(selected, targetOverrideMod.id, field.key, Number(e.target.value))
                              }
                            />
                          ) : field.kind === "color" ? (
                            <input
                              type="color"
                              value={String(displayVal || "#4de1ff")}
                              onChange={(e) =>
                                setModuleOverride(selected, targetOverrideMod.id, field.key, e.target.value)
                              }
                            />
                          ) : (
                            <input
                              type="text"
                              value={String(displayVal)}
                              style={{ width: 160 }}
                              onChange={(e) =>
                                setModuleOverride(selected, targetOverrideMod.id, field.key, e.target.value)
                              }
                            />
                          )}
                          {hasOv && (
                            <button
                              type="button"
                              className="kg-btn-ghost"
                              onClick={() => setModuleOverride(selected, targetOverrideMod.id, field.key, undefined)}
                            >
                              Reset
                            </button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </div>
        )}
      </div>

      {statusMsg && <div className="kg-muted">{statusMsg}</div>}
    </div>
  );
};
