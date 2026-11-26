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
  initialSelectedId?: string | null;
  onActiveChanged?: () => void;
}

interface RecentAppEntry {
  app_id: string;
  process_name: string;
  window_title: string;
  last_seen_ms: number;
}

interface ProfileConflictInfo {
  profile_id: string;
  profile_name: string;
  matched_rule: string;
  match_kind: string;
}

interface ProfileMatchDiagnostics {
  profile_id: string;
  profile_name: string;
  pinned: boolean;
  reason: string;
  matched_rule?: string | null;
  match_kind: string;
  evaluated_app_id: string;
  evaluated_process: string;
  current_app_is_studio: boolean;
  conflicts?: ProfileConflictInfo[];
}

function isStudioToken(tok: string): boolean {
  const low = tok.trim().toLowerCase();
  return (
    low === "org.openflux.studio" ||
    low.startsWith("org.keyglass.") ||
    low === "openflux-studio" ||
    low === "openflux studio" ||
    low === "keyglass studio" ||
    low === "keyglass"
  );
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

export const ProfilesView: React.FC<ProfilesViewProps> = ({
  modules,
  initialSelectedId,
  onActiveChanged,
}) => {
  const [profiles, setProfiles] = useState<ProfileEntry[]>([]);
  const [manualId, setManualId] = useState<string | null>(null);
  const [active, setActive] = useState<ActiveProfileState | null>(null);
  const [host, setHost] = useState<HostIdentity | null>(null);
  const [hostNameDraft, setHostNameDraft] = useState("");
  const [recentApps, setRecentApps] = useState<RecentAppEntry[]>([]);
  const [activeMatch, setActiveMatch] = useState<ProfileMatchDiagnostics | null>(null);
  const [selectedId, setSelectedId] = useState<string>(initialSelectedId || "default");
  const [overrideModId, setOverrideModId] = useState<string>("bg-media");
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  useEffect(() => {
    if (initialSelectedId) setSelectedId(initialSelectedId);
  }, [initialSelectedId]);

  const slotWidgets = modules.filter((m) => m.manifest.slot_capable);
  const configurableMods = modules.filter((m) => m.manifest.settings.length > 0);

  const loadAll = useCallback(async () => {
    try {
      const res = (await v2Client.getProfiles()) as Awaited<ReturnType<typeof v2Client.getProfiles>> & {
        recent_apps?: RecentAppEntry[];
        active_match?: ProfileMatchDiagnostics | null;
      };
      setProfiles(res.profiles ?? []);
      setManualId(res.manual_profile_id ?? null);
      setActive(res.active ?? null);
      setRecentApps(res.recent_apps ?? []);
      const am =
        res.active_match ??
        ((res.active as unknown as { active_match?: ProfileMatchDiagnostics | null })?.active_match ?? null);
      setActiveMatch(am);
      if (res.host) {
        setHost(res.host);
        setHostNameDraft((prev) => (prev ? prev : res.host.host_name));
      }
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
      onActiveChanged?.();
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
    const recent = recentApps[0];
    const defaultToken = recent ? recent.app_id || recent.process_name : "";
    const nextP: ProfileEntry = {
      id: slug,
      name: recent ? recent.process_name || recent.app_id : "New Profile",
      parent: "default",
      enabled: true,
      match_apps: defaultToken ? [defaultToken] : [],
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

  const handleCreateComputerProfile = () => {
    if (!host) return;
    const slug = `host_${host.host_id.toLowerCase().replace(/[^a-z0-9]+/g, "_")}`;
    if (profiles.some((p) => p.id === slug)) {
      setSelectedId(slug);
      return;
    }
    const nextP: ProfileEntry = {
      id: slug,
      name: host.host_name || "This Computer",
      parent: "default",
      enabled: true,
      match_apps: [],
      match_hosts: [host.host_name || host.host_id],
      match_os: [host.os],
      tags: [],
      qmk_layer: null,
      top_bar_slots: null,
      module_overrides: {},
    };
    setSelectedId(slug);
    void persist(manualId, [...profiles, nextP], `Added computer override for "${nextP.name}".`);
  };

  const resetBuiltinOsProfile = (id: string) => {
    const next = profiles.map((p) =>
      p.id === id
        ? {
            ...p,
            enabled: true,
            qmk_layer: null,
            top_bar_slots: null,
            module_overrides: {},
            match_apps: [],
            match_hosts: [],
          }
        : p,
    );
    const nextManual = manualId === id ? null : manualId;
    void persist(nextManual, next, `Reset ${id === "macos" ? "macOS" : "Windows"} back to automatic legends & shortcuts.`);
  };

  const handleSelectManual = async (pid: string | null) => {
    try {
      await v2Client.selectProfile(pid);
      setManualId(pid);
      await loadAll();
      onActiveChanged?.();
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
          <span className="kg-pill-subtle">Shared → OS → Computer → App</span>
        </div>
        <div className="kg-header-actions">
          {manualId && (
            <button type="button" className="kg-btn-ghost" onClick={() => void handleSelectManual(null)}>
              Return to Auto
            </button>
          )}
          <button type="button" className="kg-btn-primary" onClick={handleCreateProfile}>
            + New App Profile
          </button>
        </div>
      </div>

      {/* Live Resolved Overlay Stack Card (Left-to-Right: Shared -> OS -> Computer -> App) */}
      {active && (
        <div
          className="kg-calm-card"
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 10,
            borderColor: manualId ? "rgba(92, 225, 255, 0.45)" : undefined,
          }}
        >
          <div className="kg-calm-row" style={{ flexWrap: "wrap" }}>
            <div>
              <strong>{manualId ? "📌 Pinned Overlay Stack:" : "Live Overlay Stack:"}</strong>
              <div className="kg-muted" style={{ fontSize: 12, marginTop: 3 }}>
                {activeMatch?.reason || `Source: ${active.source}`}
                {activeMatch?.evaluated_app_id && !activeMatch.current_app_is_studio && (
                  <>
                    {" "}
                    · App ID: <code>{activeMatch.evaluated_app_id}</code>
                  </>
                )}
              </div>
            </div>
            <div style={{ display: "flex", gap: 8 }}>
              {manualId ? (
                <button type="button" className="kg-btn-primary" onClick={() => void handleSelectManual(null)}>
                  Return to Automatic
                </button>
              ) : (
                selected && selected.id !== "default" && (
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    onClick={() => void handleSelectManual(selected.id)}
                  >
                    Pin “{selected.name}”
                  </button>
                )
              )}
            </div>
          </div>

          {Array.isArray(active.stack) && active.stack.length > 0 && (
            <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 6 }}>
              {active.stack.map((layer, idx) => (
                <div key={`${layer.scope}-${layer.id}`} style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  {idx > 0 && <span className="kg-muted" style={{ fontSize: 12 }}>→</span>}
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    onClick={() => setSelectedId(layer.id)}
                    style={{
                      padding: "4px 10px",
                      fontSize: 12,
                      borderColor:
                        selected?.id === layer.id
                          ? "rgba(92, 225, 255, 0.55)"
                          : layer.pinned
                            ? "rgba(92, 225, 255, 0.4)"
                            : undefined,
                    }}
                    title={layer.summary}
                  >
                    <span style={{ fontWeight: 600 }}>
                      {layer.pinned ? "📌 " : ""}
                      {layer.name}
                    </span>
                    <span className="kg-muted" style={{ marginLeft: 6, fontSize: 11 }}>
                      {layer.summary}
                    </span>
                  </button>
                </div>
              ))}
            </div>
          )}
          {activeMatch?.conflicts && activeMatch.conflicts.length > 0 && (
            <div
              style={{
                padding: "8px 10px",
                borderRadius: 8,
                background: "rgba(251, 191, 36, 0.10)",
                border: "1px solid rgba(251, 191, 36, 0.35)",
                fontSize: 12,
              }}
            >
              <strong style={{ color: "#fbbf24" }}>Competing profiles also matched this app:</strong>{" "}
              {activeMatch.conflicts.map((c, i) => (
                <span key={c.profile_id}>
                  {i > 0 ? " · " : ""}
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    style={{ padding: "1px 6px", fontSize: 11 }}
                    onClick={() => setSelectedId(c.profile_id)}
                  >
                    {c.profile_name} ({c.match_kind}: “{c.matched_rule}”)
                  </button>
                </span>
              ))}
              <div className="kg-muted" style={{ fontSize: 11, marginTop: 4 }}>
                Specificity order: Exact App ID &gt; Exact Process &gt; Broad Substring. Use Exact App IDs below to
                prevent overlaps.
              </div>
            </div>
          )}
        </div>
      )}

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

      <div style={{ display: "grid", gridTemplateColumns: "264px minmax(0, 1fr)", gap: 16, alignItems: "start" }}>
        {/* Grouped Profiles List: Shared Baseline / OS & Computers / Apps & Activities */}
        {(() => {
          const stackIds = new Set((active?.stack ?? []).map((s) => s.id));
          const isOsOrHost = (p: ProfileEntry) =>
            p.id !== "default" &&
            p.match_apps.length === 0 &&
            (p.match_os.length > 0 || p.match_hosts.length > 0);
          const baseProfiles = profiles.filter((p) => p.id === "default");
          const envProfiles = profiles.filter(isOsOrHost);
          const appProfiles = profiles.filter((p) => p.id !== "default" && !isOsOrHost(p));

          const renderItem = (p: ProfileEntry) => {
            const isSel = selected?.id === p.id;
            const inLiveStack = stackIds.has(p.id) || active?.profile_id === p.id;
            const isPinned = manualId === p.id;
            const ovCount = Object.values(p.module_overrides ?? {}).reduce(
              (acc, m) => acc + Object.keys(m ?? {}).length,
              0,
            );
            const subtitle =
              p.id === "default"
                ? "All computers & apps"
                : p.implicit
                  ? p.id === "windows"
                    ? "Auto · Win/Alt & Ctrl"
                    : "Auto · Cmd/Opt & Cmd"
                  : [
                      p.match_hosts.length > 0 ? p.match_hosts[0] : null,
                      p.qmk_layer != null ? `L${p.qmk_layer}` : null,
                      p.top_bar_slots ? "Top Bar" : null,
                      ovCount > 0 ? `${ovCount} override${ovCount === 1 ? "" : "s"}` : null,
                    ]
                      .filter(Boolean)
                      .join(" · ") || "Inherits all settings";
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
                    {subtitle}
                  </div>
                </span>
                {inLiveStack && <span className="kg-conn-dot is-on" title="Active in current overlay stack" />}
              </button>
            );
          };

          return (
            <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 6, padding: 10 }}>
              <div className="kg-muted" style={{ fontSize: 10, fontWeight: 700, letterSpacing: "0.08em", textTransform: "uppercase", padding: "2px 6px" }}>
                Shared Baseline
              </div>
              {baseProfiles.map(renderItem)}

              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginTop: 8,
                  padding: "2px 6px",
                }}
              >
                <span className="kg-muted" style={{ fontSize: 10, fontWeight: 700, letterSpacing: "0.08em", textTransform: "uppercase" }}>
                  OS &amp; Computers
                </span>
                {host && (
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    style={{ padding: "1px 6px", fontSize: 10 }}
                    onClick={handleCreateComputerProfile}
                    title={`Add per-computer override for ${host.host_name}`}
                  >
                    + This PC
                  </button>
                )}
              </div>
              {envProfiles.map(renderItem)}

              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginTop: 8,
                  padding: "2px 6px",
                }}
              >
                <span className="kg-muted" style={{ fontSize: 10, fontWeight: 700, letterSpacing: "0.08em", textTransform: "uppercase" }}>
                  Apps &amp; Activities
                </span>
                <button
                  type="button"
                  className="kg-btn-ghost"
                  style={{ padding: "1px 6px", fontSize: 10 }}
                  onClick={handleCreateProfile}
                >
                  + App
                </button>
              </div>
              {appProfiles.length === 0 ? (
                <div className="kg-muted" style={{ fontSize: 11, padding: "4px 8px" }}>
                  No app profiles yet. App profiles automatically stack on top of macOS or Windows.
                </div>
              ) : (
                appProfiles.map(renderItem)
              )}
            </div>
          );
        })()}

        {/* Selected Profile Editor */}
        {selected && (
          <div className="kg-calm-card kg-settings-stack">
            <div className="kg-calm-row" style={{ flexWrap: "wrap" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <div className="kg-row-title">{selected.name}</div>
                <span className="kg-pill-subtle">
                  {selected.id === "default"
                    ? "Shared baseline"
                    : selected.implicit
                      ? "Built-in OS (auto)"
                      : `id: ${selected.id}`}
                </span>
              </div>
              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <button
                  type="button"
                  className={manualId === selected.id ? "kg-btn-primary" : "kg-btn-ghost"}
                  onClick={() => void handleSelectManual(manualId === selected.id ? null : selected.id)}
                >
                  {manualId === selected.id ? "Pinned Active (Unpin)" : "Pin Active Now"}
                </button>
                {selected.id === "macos" || selected.id === "windows" ? (
                  !selected.implicit && (
                    <button
                      type="button"
                      className="kg-btn-ghost"
                      onClick={() => resetBuiltinOsProfile(selected.id)}
                    >
                      Reset to Auto
                    </button>
                  )
                ) : (
                  selected.id !== "default" && (
                    <button type="button" className="kg-btn-ghost" onClick={() => deleteProfile(selected.id)}>
                      Delete
                    </button>
                  )
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
                <div className="kg-setting-row" style={{ alignItems: "flex-start" }}>
                  <div className="kg-setting-meta">
                    <div className="kg-setting-label">Auto-Activate on Applications</div>
                    <div className="kg-muted">
                      Exact bundle/app IDs (recommended, highest priority), exact process names, or broad substrings.
                    </div>
                  </div>
                  <div className="kg-setting-ctrl" style={{ flexDirection: "column", alignItems: "stretch", gap: 8, minWidth: 320 }}>
                    <input
                      type="text"
                      style={{ width: "100%" }}
                      placeholder="e.g. com.blizzard.worldofwarcraft, Code, Cursor"
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
                    {selected.match_apps.some(isStudioToken) && (
                      <div style={{ fontSize: 11, color: "#fbbf24" }}>
                        Warning: This rule matches Keyglass Studio itself — remove it unless you intentionally want
                        this profile active while configuring Studio.
                      </div>
                    )}
                    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                      <span className="kg-muted" style={{ fontSize: 11 }}>
                        Recently focused apps (Keyglass Studio excluded automatically — click to add):
                      </span>
                      {recentApps.length === 0 ? (
                        <span className="kg-muted" style={{ fontSize: 11 }}>
                          Switch briefly to the app you want to bind, then return here and it will appear for 1-click
                          binding.
                        </span>
                      ) : (
                        <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                          {recentApps.slice(0, 6).map((app) => {
                            const exactId = app.app_id || app.process_name;
                            const label = app.process_name || app.app_id;
                            const hasExact = selected.match_apps.some(
                              (t) => t.toLowerCase() === exactId.toLowerCase(),
                            );
                            return (
                              <button
                                key={`${app.app_id}:${app.process_name}`}
                                type="button"
                                className="kg-btn-ghost"
                                style={{ fontSize: 11, padding: "3px 8px" }}
                                disabled={hasExact}
                                title={app.app_id ? `Exact App ID: ${app.app_id}` : `Process: ${app.process_name}`}
                                onClick={() => {
                                  if (!hasExact && exactId) {
                                    updateProfile(selected.id, { match_apps: [...selected.match_apps, exactId] });
                                  }
                                }}
                              >
                                {hasExact ? `✓ ${label}` : `+ ${label}${app.app_id ? ` (${app.app_id})` : ""}`}
                              </button>
                            );
                          })}
                        </div>
                      )}
                    </div>
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
