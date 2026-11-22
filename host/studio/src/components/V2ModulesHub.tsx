import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  type CuratedListSubscription,
  type InstallPreview,
  type ModuleCategory,
  type ModuleLogEntry,
  type PreviewFrameTimings,
  type V2ModuleRecord,
  v2Client,
} from "../api/v2Client";

const PINNED_TAGS = ["keys", "top-bar", "visual", "background", "companion", "3d", "prusa", "gaming", "core"];

function moduleAllTags(m: V2ModuleRecord): string[] {
  const set = new Set<string>(m.manifest.tags ?? []);
  return Array.from(set);
}

const KEY_INDEX_OPTIONS: Array<{ value: number; label: string }> = [
  { value: 0, label: "Esc (0)" },
  { value: 1, label: "F1 (1)" },
  { value: 2, label: "F2 (2)" },
  { value: 3, label: "F3 (3)" },
  { value: 4, label: "F4 (4)" },
  { value: 5, label: "F5 (5)" },
  { value: 6, label: "F6 (6)" },
  { value: 7, label: "F7 (7)" },
  { value: 8, label: "F8 (8)" },
  { value: 9, label: "F9 (9)" },
  { value: 10, label: "F10 (10)" },
  { value: 11, label: "F11 (11)" },
  { value: 12, label: "F12 (12)" },
  { value: 13, label: "Prt (13)" },
  { value: 14, label: "Ins (14)" },
  { value: 15, label: "Del (15)" },
  { value: 30, label: "Home (30)" },
  { value: 45, label: "PgUp (45)" },
  { value: 59, label: "PgDn (59)" },
  { value: 73, label: "End (73)" },
];

export interface V2ModulesHubProps {
  /** When set, focuses this dedicated single-module workspace (for module-contributed top tabs). */
  focusedModuleId?: string;
  pickFile?: (title: string) => Promise<string | null>;
  onModulesChanged?: (mods: V2ModuleRecord[]) => void;
}

export const V2ModulesHub: React.FC<V2ModulesHubProps> = ({
  focusedModuleId,
  pickFile,
  onModulesChanged,
}) => {
  const [modules, setModules] = useState<V2ModuleRecord[]>([]);
  const [lists, setLists] = useState<CuratedListSubscription[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(focusedModuleId ?? null);
  const [autoSyncKeyboard, setAutoSyncKeyboard] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const syncTimerRef = useRef<number | null>(null);

  const scheduleKeyboardSync = useCallback(() => {
    if (!autoSyncKeyboard) return;
    if (syncTimerRef.current !== null) {
      window.clearTimeout(syncTimerRef.current);
    }
    syncTimerRef.current = window.setTimeout(() => {
      syncTimerRef.current = null;
      void v2Client.syncToKeyboard().catch(() => {
        // Ignore if keyboard is unplugged
      });
    }, 280);
  }, [autoSyncKeyboard]);

  // Live GPU preview + reactive sync state
  const [previewPng, setPreviewPng] = useState<string | null>(null);
  const [gpuAdapter, setGpuAdapter] = useState<string>("");
  const [timings, setTimings] = useState<PreviewFrameTimings | null>(null);
  const [previewRunning, setPreviewRunning] = useState<boolean>(true);
  const [selectedLogs, setSelectedLogs] = useState<ModuleLogEntry[]>([]);
  const inflightRef = useRef(false);
  const modulesRevRef = useRef<number>(0);

  // Disclosure install modal state
  const [installPath, setInstallPath] = useState("");
  const [installPreview, setInstallPreview] = useState<InstallPreview | null>(null);

  // Scaffold new local module state
  const [newModId, setNewModId] = useState("");
  const [newModCat, setNewModCat] = useState<ModuleCategory>("effect");

  const refresh = useCallback(async () => {
    try {
      const [mRes, lRes] = await Promise.all([v2Client.listModules(), v2Client.listCuratedLists()]);
      setModules(mRes.modules);
      onModulesChanged?.(mRes.modules);
      setLists(lRes.lists);
      if (typeof mRes.modules_rev === "number") {
        modulesRevRef.current = mRes.modules_rev;
      }
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [onModulesChanged]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Live reactive bus subscription (`system/modules/#`): whenever an AI agent or CLI modifies
  // modules, configs, or files on disk, Studio updates immediately—even when preview is paused!
  useEffect(() => {
    let alive = true;
    let sinceSeq = 0;
    const runLoop = async () => {
      try {
        const st = await v2Client.getStatus();
        sinceSeq = st.latest_bus_seq;
      } catch {
        // Retry in loop if daemon is booting.
      }
      while (alive) {
        try {
          const res = await v2Client.pollBusEvents("system/modules/#", sinceSeq, 2500);
          if (!alive) break;
          sinceSeq = res.latest_seq;
          if (res.events.length > 0) {
            await refresh();
          }
        } catch {
          await new Promise((r) => window.setTimeout(r, 1000));
        }
      }
    };
    void runLoop();
    return () => {
      alive = false;
    };
  }, [refresh]);

  // Live GPU compositor preview loop (~12 fps) + instant rev sync + perf timings
  useEffect(() => {
    if (!previewRunning) return;
    let alive = true;
    const tick = async () => {
      if (!alive || inflightRef.current) return;
      inflightRef.current = true;
      try {
        const res = await v2Client.renderPreviewFrame(2, 16.667, 960, 540);
        if (alive) {
          setPreviewPng(`data:image/png;base64,${res.png_base64}`);
          setGpuAdapter(res.gpu_adapter);
          if (res.timings) setTimings(res.timings);
          if (typeof res.modules_rev === "number" && res.modules_rev !== modulesRevRef.current) {
            modulesRevRef.current = res.modules_rev;
            void refresh();
          }
        }
      } catch {
        // Daemon may be offline in unit tests; stay quiet.
      } finally {
        inflightRef.current = false;
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 90);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [previewRunning, refresh]);

  // Live logs refresh for the selected module (updates when AI edits or module logs)
  useEffect(() => {
    const targetId = selectedId ?? modules[0]?.id;
    if (!targetId) {
      setSelectedLogs([]);
      return;
    }
    let alive = true;
    const pull = async () => {
      try {
        const res = await v2Client.getModuleLogs(targetId);
        if (alive) setSelectedLogs(res.logs);
      } catch {
        // ignore
      }
    };
    void pull();
    const timer = window.setInterval(() => void pull(), 1200);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [selectedId, modules]);

  const handleToggle = async (mod: V2ModuleRecord) => {
    try {
      await v2Client.updateModuleState(mod.id, { enabled: !mod.state.enabled });
      await refresh();
      scheduleKeyboardSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleSettingChange = async (mod: V2ModuleRecord, key: string, value: unknown) => {
    // Immediate optimistic update so sliders & text inputs feel silky-smooth
    setModules((prev) =>
      prev.map((m) =>
        m.id === mod.id
          ? { ...m, state: { ...m.state, config: { ...m.state.config, [key]: value } } }
          : m,
      ),
    );
    try {
      await v2Client.updateModuleState(mod.id, { config: { [key]: value } });
      scheduleKeyboardSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleFork = async (mod: V2ModuleRecord) => {
    const newId = `${mod.id}-custom-${Date.now().toString().slice(-4)}`;
    try {
      await v2Client.forkModule({ source_id: mod.id, new_id: newId, enable: true });
      setNotice(`Forked ${mod.manifest.name} into local module "${newId}" (safe from upstream updates).`);
      await refresh();
      setSelectedId(newId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleCreateLocal = async (e: React.FormEvent) => {
    e.preventDefault();
    const id = newModId.trim().toLowerCase();
    if (!id) return;
    try {
      await v2Client.createModule({ id, category: newModCat, enable: true });
      setNewModId("");
      setNotice(`Created local plain-folder module "${id}" ready for live editing & hot-reload.`);
      setActiveTag(null);
      await refresh();
      setSelectedId(id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleInspectInstall = async () => {
    if (!installPath.trim()) return;
    try {
      const res = await v2Client.inspectModule(installPath.trim());
      setInstallPreview(res.preview);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleConfirmInstall = async () => {
    if (!installPath.trim()) return;
    try {
      await v2Client.installModule(installPath.trim(), true);
      setInstallPreview(null);
      setInstallPath("");
      setNotice("Module installed after disclosure acknowledgement.");
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleCanvasClick = async (e: React.MouseEvent<HTMLImageElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * 1920;
    const y = ((e.clientY - rect.top) / rect.height) * 1080;
    await v2Client.simulateImpulse(x, y, 64, 1.25);
  };

  const handleExportPreset = async () => {
    try {
      const res = await v2Client.exportPreset("My OpenFlux v2 Setup", "Exported from OpenFlux Studio");
      const blob = new Blob([JSON.stringify(res.preset, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "openflux-v2-preset.json";
      a.click();
      URL.revokeObjectURL(url);
      setNotice("Exported secret-free setup preset JSON.");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleTogglePinTab = async (mod: V2ModuleRecord) => {
    const currentlyPinned = mod.state.pin_tab ?? Boolean(mod.manifest.ui?.top_tab);
    try {
      await v2Client.updateModuleState(mod.id, { pin_tab: !currentlyPinned });
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleTriggerAction = async (mod: V2ModuleRecord, actionId: string, label: string) => {
    try {
      await v2Client.triggerAction(mod.id, actionId);
      setNotice(`Triggered "${label}" on ${mod.manifest.name}`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  // Single unified tag list (popular/core tags first, then alphabetical — no weird super-tags!)
  const allTags = Array.from(new Set(modules.flatMap(moduleAllTags))).sort((a, b) => {
    const ia = PINNED_TAGS.indexOf(a);
    const ib = PINNED_TAGS.indexOf(b);
    if (ia !== -1 && ib !== -1) return ia - ib;
    if (ia !== -1) return -1;
    if (ib !== -1) return 1;
    return a.localeCompare(b);
  });

  const q = searchQuery.trim().toLowerCase();
  const filtered = modules.filter((m) => {
    if (focusedModuleId) return m.id === focusedModuleId;
    if (activeTag && !moduleAllTags(m).includes(activeTag)) return false;
    if (q) {
      const hay = `${m.id} ${m.manifest.name} ${m.manifest.description} ${moduleAllTags(m).join(" ")}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
  const selected = focusedModuleId
    ? (modules.find((m) => m.id === focusedModuleId) ?? null)
    : (filtered.find((m) => m.id === selectedId) ?? filtered[0] ?? null);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16, padding: 16 }}>
      {/* Live Unified GPU Compositor Preview */}
      <section
        style={{
          background: "rgba(14, 18, 28, 0.9)",
          border: "1px solid rgba(110, 180, 255, 0.25)",
          borderRadius: 12,
          padding: 14,
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10, flexWrap: "wrap", gap: 8 }}>
          <div>
            <strong>OpenFlux v2 Live GPU Compositor Preview</strong>
            {gpuAdapter && (
              <span style={{ marginLeft: 10, fontSize: 12, opacity: 0.72 }}>GPU: {gpuAdapter}</span>
            )}
            {timings && (
              <span style={{ marginLeft: 12, fontSize: 12, color: "#7ce8ff" }}>
                {timings.total_frame_ms.toFixed(1)} ms/frame (Lua {timings.lua_ms.toFixed(2)} · Phys {timings.physics_ms.toFixed(2)} · GPU {timings.gpu_submit_ms.toFixed(2)}) · {timings.particle_count} particles · Water {timings.wave_enabled ? "ON" : "OFF"}
              </span>
            )}
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <button type="button" onClick={() => void v2Client.simulateBurst()}>
              Physics + Water Burst
            </button>
            <button type="button" onClick={() => void v2Client.simulateKey(49, 2500, 16500, false)}>
              Press Down (D)
            </button>
            <button type="button" onClick={() => void v2Client.simulateKey(53, 1800, -13500, true)}>
              Lift Up (J)
            </button>
            <button
              type="button"
              onClick={() =>
                void v2Client.publishBus("modules/companion-idle-pet/command", { action: "roam", target_key: 38 })
              }
            >
              Send Pet Roaming
            </button>
            <button type="button" onClick={() => setPreviewRunning((r) => !r)}>
              {previewRunning ? "Pause Preview" : "Resume Preview"}
            </button>
            <button type="button" onClick={() => void handleExportPreset()}>
              Export Preset
            </button>
            <label style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 12 }}>
              <input
                type="checkbox"
                checked={autoSyncKeyboard}
                onChange={(e) => setAutoSyncKeyboard(e.target.checked)}
              />
              Auto-Sync Keyboard
            </label>
            <button
              type="button"
              style={{ background: "#1ea7d8", color: "#fff", border: "none", padding: "6px 12px", borderRadius: 6 }}
              onClick={async () => {
                try {
                  setNotice("Syncing v2 modules & media to keyboard…");
                  const r = await v2Client.syncToKeyboard();
                  setNotice(`Synced to keyboard (${r.uploaded_files.length} files, ${(r.uploaded_bytes / 1024).toFixed(1)} KB).`);
                } catch (err) {
                  setError(err instanceof Error ? err.message : String(err));
                }
              }}
            >
              Sync to Keyboard
            </button>
          </div>
        </div>

        <div style={{ position: "relative", width: "100%", maxWidth: 960, margin: "0 auto" }}>
          {previewPng ? (
            <img
              src={previewPng}
              alt="OpenFlux v2 GPU Preview (click to ripple)"
              onClick={(e) => void handleCanvasClick(e)}
              style={{
                width: "100%",
                aspectRatio: "16 / 9",
                borderRadius: 10,
                cursor: "crosshair",
                display: "block",
                border: "1px solid rgba(255,255,255,0.12)",
              }}
            />
          ) : (
            <div
              style={{
                width: "100%",
                aspectRatio: "16 / 9",
                borderRadius: 10,
                background: "#090d16",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                opacity: 0.7,
              }}
            >
              Connecting to v2 GPU compositor…
            </div>
          )}
          <div style={{ fontSize: 12, opacity: 0.65, marginTop: 6, textAlign: "center" }}>
            Click anywhere on the keyboard preview to inject a live <code>fx/surface/impulse</code> across all active modules.
          </div>
        </div>
      </section>

      {error && (
        <div style={{ padding: 10, borderRadius: 8, background: "rgba(220, 48, 64, 0.2)", color: "#ff9ca6" }}>
          {error}
        </div>
      )}
      {notice && (
        <div style={{ padding: 10, borderRadius: 8, background: "rgba(48, 200, 140, 0.16)", color: "#8ef5c8" }}>
          {notice}
        </div>
      )}

      {/* Single Unified Tag Filter Bar + Search (hidden in focused single-module top tab) */}
      {!focusedModuleId && (
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <button
            type="button"
            onClick={() => setActiveTag(null)}
            style={{
              fontSize: 12,
              padding: "6px 12px",
              borderRadius: 99,
              border: activeTag === null ? "1px solid #4de1ff" : "1px solid rgba(255,255,255,0.14)",
              background: activeTag === null ? "rgba(77, 225, 255, 0.18)" : "rgba(20, 24, 34, 0.7)",
              color: "#fff",
              cursor: "pointer",
            }}
          >
            All ({modules.length})
          </button>
          {allTags.map((tag) => {
            const cnt = modules.filter((m) => moduleAllTags(m).includes(tag)).length;
            return (
              <button
                key={tag}
                type="button"
                onClick={() => setActiveTag(activeTag === tag ? null : tag)}
                style={{
                  fontSize: 12,
                  padding: "6px 11px",
                  borderRadius: 99,
                  border: activeTag === tag ? "1px solid #4de1ff" : "1px solid rgba(255,255,255,0.12)",
                  background: activeTag === tag ? "rgba(77, 225, 255, 0.18)" : "rgba(255,255,255,0.04)",
                  color: "#dbe7f5",
                  cursor: "pointer",
                }}
              >
                #{tag} ({cnt})
              </button>
            );
          })}
          <input
            type="search"
            placeholder="Search modules…"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            style={{
              marginLeft: "auto",
              minWidth: 210,
              padding: "7px 10px",
              borderRadius: 8,
              border: "1px solid rgba(255,255,255,0.16)",
              background: "rgba(12, 16, 24, 0.85)",
              color: "#fff",
            }}
          />
        </div>
      )}

      {/* Module List + Inspector / Settings */}
      <div style={{ display: "grid", gridTemplateColumns: focusedModuleId ? "1fr" : "1.2fr 1fr", gap: 16 }}>
        {!focusedModuleId && (
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          {filtered.map((mod) => (
            <div
              key={mod.id}
              onClick={() => setSelectedId(mod.id)}
              style={{
                padding: 12,
                borderRadius: 10,
                border:
                  selected?.id === mod.id
                    ? "1px solid rgba(77, 225, 255, 0.7)"
                    : "1px solid rgba(255, 255, 255, 0.12)",
                background: "rgba(18, 22, 32, 0.85)",
                cursor: "pointer",
              }}
            >
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <div>
                  <strong>{mod.manifest.name}</strong>{" "}
                  <span style={{ fontSize: 11, opacity: 0.65 }}>
                    v{mod.manifest.version} · {mod.tier} ·{" "}
                    {mod.manifest.host_requirement === "none"
                      ? "Standalone"
                      : mod.manifest.host_requirement === "optional"
                        ? "Hybrid"
                        : "Host-Driven"}
                  </span>
                </div>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }} onClick={(e) => e.stopPropagation()}>
                  <button type="button" onClick={() => void handleToggle(mod)}>
                    {mod.state.enabled ? "Enabled" : "Disabled"}
                  </button>
                </div>
              </div>
              <div style={{ fontSize: 12, opacity: 0.78, marginTop: 6 }}>{mod.manifest.description}</div>
              {(mod.manifest.tags ?? []).length > 0 && (
                <div style={{ display: "flex", gap: 5, flexWrap: "wrap", marginTop: 6 }}>
                  {(mod.manifest.tags ?? []).map((t) => (
                    <span
                      key={t}
                      style={{
                        fontSize: 10,
                        padding: "2px 6px",
                        borderRadius: 99,
                        background: "rgba(255,255,255,0.07)",
                        color: "#a8c5e5",
                      }}
                    >
                      #{t}
                    </span>
                  ))}
                </div>
              )}
              {!mod.healthy && mod.last_error && (
                <div style={{ fontSize: 12, color: "#ff8c96", marginTop: 6 }}>Runtime Error: {mod.last_error}</div>
              )}
            </div>
          ))}
        </div>
        )}

        {/* Selected Module Details, Settings & Fork */}
        <div
          style={{
            padding: 14,
            borderRadius: 10,
            border: "1px solid rgba(255,255,255,0.12)",
            background: "rgba(18, 22, 32, 0.85)",
            display: "flex",
            flexDirection: "column",
            gap: 12,
          }}
        >
          {selected ? (
            <>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <div>
                  <h3 style={{ margin: 0 }}>{selected.manifest.name}</h3>
                  <div style={{ fontSize: 12, opacity: 0.65 }}>
                    <code>{selected.id}</code> · {selected.path}
                  </div>
                </div>
                <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                  <button type="button" onClick={() => void handleTogglePinTab(selected)}>
                    {(selected.state.pin_tab ?? Boolean(selected.manifest.ui?.top_tab)) ? "Unpin Top Tab" : "Pin as Top Tab"}
                  </button>
                  <button type="button" onClick={() => void handleFork(selected)}>
                    Fork to Local
                  </button>
                  <button type="button" onClick={() => void v2Client.reloadModule(selected.id)}>
                    Hot Reload
                  </button>
                </div>
              </div>

              {(selected.manifest.actions ?? []).length > 0 && (
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
                  <strong style={{ fontSize: 12 }}>Actions:</strong>
                  {(selected.manifest.actions ?? []).map((act) => (
                    <button
                      key={act.id}
                      type="button"
                      onClick={() => void handleTriggerAction(selected, act.id, act.label)}
                    >
                      {act.label}
                    </button>
                  ))}
                </div>
              )}

              {selected.manifest.settings.length > 0 ? (
                <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                  <strong>Settings</strong>
                  {selected.manifest.settings.map((field) => {
                    const cur = selected.state.config[field.key] ?? field.default ?? "";
                    const maxVal = field.max ?? 100;
                    const stepVal = maxVal <= 1.05 ? 0.002 : maxVal <= 5 ? 0.05 : 1;
                    return (
                      <label key={field.key} style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 13 }}>
                        <span>
                          {field.label} {field.secret && <em style={{ opacity: 0.6 }}>(secret — excluded from presets)</em>}
                        </span>
                        {field.kind === "boolean" ? (
                          <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                            <input
                              type="checkbox"
                              checked={Boolean(cur)}
                              onChange={(e) => void handleSettingChange(selected, field.key, e.target.checked)}
                            />
                            <span style={{ fontSize: 12, opacity: 0.8 }}>{Boolean(cur) ? "On" : "Off"}</span>
                          </span>
                        ) : field.kind === "select" ? (
                          <select
                            value={String(cur)}
                            onChange={(e) => void handleSettingChange(selected, field.key, e.target.value)}
                          >
                            {(field.options ?? []).map((opt) => (
                              <option key={opt} value={opt}>
                                {opt}
                              </option>
                            ))}
                          </select>
                        ) : field.kind === "key" ? (
                          <select
                            value={Number(cur) || 0}
                            onChange={(e) => void handleSettingChange(selected, field.key, Number(e.target.value))}
                          >
                            {KEY_INDEX_OPTIONS.map((k) => (
                              <option key={k.value} value={k.value}>
                                {k.label}
                              </option>
                            ))}
                          </select>
                        ) : field.kind === "file" ? (
                          <div style={{ display: "flex", gap: 6 }}>
                            <input
                              type="text"
                              value={String(cur)}
                              onChange={(e) => void handleSettingChange(selected, field.key, e.target.value)}
                              style={{ flex: 1 }}
                            />
                            {pickFile && (
                              <button
                                type="button"
                                onClick={async () => {
                                  const picked = await pickFile(`Select ${field.label}`);
                                  if (picked) {
                                    await handleSettingChange(selected, field.key, picked);
                                  }
                                }}
                              >
                                Browse…
                              </button>
                            )}
                          </div>
                        ) : field.kind === "color" ? (
                          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                            <input
                              type="color"
                              value={String(cur || "#4de1ff")}
                              onChange={(e) => void handleSettingChange(selected, field.key, e.target.value)}
                            />
                            <code style={{ fontSize: 12 }}>{String(cur || "#4de1ff")}</code>
                          </div>
                        ) : field.kind === "number" ? (
                          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                            <input
                              type="range"
                              min={field.min ?? 0}
                              max={maxVal}
                              step={stepVal}
                              value={Number(cur) || 0}
                              onChange={(e) => void handleSettingChange(selected, field.key, Number(e.target.value))}
                              style={{ flex: 1 }}
                            />
                            <input
                              type="number"
                              min={field.min ?? 0}
                              max={maxVal}
                              step={stepVal}
                              value={Number(cur) || 0}
                              onChange={(e) => void handleSettingChange(selected, field.key, Number(e.target.value))}
                              style={{ width: 74, textAlign: "right" }}
                            />
                          </div>
                        ) : (
                          <input
                            type={field.secret ? "password" : "text"}
                            value={String(cur)}
                            onChange={(e) => void handleSettingChange(selected, field.key, e.target.value)}
                          />
                        )}
                      </label>
                    );
                  })}
                </div>
              ) : (
                <div style={{ fontSize: 12, opacity: 0.65 }}>No configurable settings declared.</div>
              )}

              <div style={{ fontSize: 12, opacity: 0.72 }}>
                <div>
                  <strong>Stages:</strong> {selected.manifest.stages.join(", ") || "none"}
                </div>
                <div>
                  <strong>Subscribes:</strong> {selected.manifest.channels.subscribes.join(", ") || "none"}
                </div>
                <div>
                  <strong>Publishes:</strong> {selected.manifest.channels.publishes.join(", ") || "none"}
                </div>
              </div>

              <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                <strong style={{ fontSize: 12 }}>Live Module Logs ({selectedLogs.length})</strong>
                <div
                  style={{
                    maxHeight: 120,
                    overflowY: "auto",
                    background: "rgba(6, 8, 14, 0.85)",
                    borderRadius: 6,
                    padding: 8,
                    fontFamily: "monospace",
                    fontSize: 11,
                    color: "#c8d6e5",
                  }}
                >
                  {selectedLogs.length === 0 ? (
                    <span style={{ opacity: 0.55 }}>No logs or errors yet. Use module.log(&quot;info&quot;, &quot;...&quot;) in Lua or stdout in host scripts.</span>
                  ) : (
                    selectedLogs.slice(-20).map((entry, idx) => (
                      <div
                        key={`${entry.timestamp_ms}-${idx}`}
                        style={{ color: entry.level === "error" || entry.level === "stderr" ? "#ff8c96" : "#c8d6e5" }}
                      >
                        [{entry.level}] {entry.message}
                      </div>
                    ))
                  )}
                </div>
              </div>
            </>
          ) : (
            <div style={{ opacity: 0.65 }}>Select a module to inspect or configure.</div>
          )}
        </div>
      </div>

      {/* Create Local Module + Install with Disclosure + Curated Lists */}
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
        <form
          onSubmit={(e) => void handleCreateLocal(e)}
          style={{
            padding: 14,
            borderRadius: 10,
            border: "1px solid rgba(255,255,255,0.12)",
            background: "rgba(18, 22, 32, 0.85)",
            display: "flex",
            flexDirection: "column",
            gap: 8,
          }}
        >
          <strong>Create Local Module (Zero-Upload AI/Local Authoring)</strong>
          <div style={{ display: "flex", gap: 8 }}>
            <input
              placeholder="module-id (e.g. git-branch-tile)"
              value={newModId}
              onChange={(e) => setNewModId(e.target.value)}
              style={{ flex: 1 }}
            />
            <select value={newModCat} onChange={(e) => setNewModCat(e.target.value as ModuleCategory)}>
              <option value="effect">Effect</option>
              <option value="widget">Widget</option>
              <option value="companion">Companion</option>
              <option value="background">Background</option>
              <option value="host_service">Host Service</option>
            </select>
            <button type="submit">Scaffold</button>
          </div>
        </form>

        <div
          style={{
            padding: 14,
            borderRadius: 10,
            border: "1px solid rgba(255,255,255,0.12)",
            background: "rgba(18, 22, 32, 0.85)",
            display: "flex",
            flexDirection: "column",
            gap: 8,
          }}
        >
          <strong>Install / Update Module Folder (Transparent Disclosure)</strong>
          <div style={{ display: "flex", gap: 8 }}>
            <input
              placeholder="/path/to/module-folder"
              value={installPath}
              onChange={(e) => setInstallPath(e.target.value)}
              style={{ flex: 1 }}
            />
            <button type="button" onClick={() => void handleInspectInstall()}>
              Inspect & Install
            </button>
          </div>
          <div style={{ fontSize: 12, opacity: 0.7 }}>
            Subscribed Curated Lists: {lists.map((l) => l.name).join(", ") || "None"}
          </div>
        </div>
      </div>

      {/* Transparent Install / Update Disclosure Popup Modal */}
      {installPreview && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            background: "rgba(0, 0, 0, 0.72)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            zIndex: 1000,
          }}
        >
          <div
            style={{
              width: "min(640px, 92vw)",
              maxHeight: "82vh",
              overflowY: "auto",
              background: "#121722",
              border: "1px solid rgba(77, 225, 255, 0.45)",
              borderRadius: 14,
              padding: 20,
              display: "flex",
              flexDirection: "column",
              gap: 12,
            }}
          >
            <h3 style={{ margin: 0 }}>
              {installPreview.update_diff ? "Module Update Disclosure" : "Module Install Disclosure"}:{" "}
              {installPreview.disclosure.name}
            </h3>
            <div style={{ fontSize: 13, opacity: 0.85 }}>
              OpenFlux modules are not capability-sandboxed. Review what this bundle contains before installing:
            </div>
            <ul style={{ margin: 0, paddingLeft: 20, fontSize: 13 }}>
              {installPreview.disclosure.summary_lines.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
            {installPreview.disclosure.warnings.length > 0 && (
              <div style={{ padding: 10, borderRadius: 8, background: "rgba(255, 170, 50, 0.16)", color: "#ffd27d" }}>
                <strong>Attention:</strong>
                <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                  {installPreview.disclosure.warnings.map((w) => (
                    <li key={w}>{w}</li>
                  ))}
                </ul>
              </div>
            )}
            {installPreview.update_diff && (
              <div style={{ padding: 10, borderRadius: 8, background: "rgba(77, 225, 255, 0.12)" }}>
                <strong>What changed in this update:</strong>
                <ul style={{ margin: "4px 0 0", paddingLeft: 18, fontSize: 12 }}>
                  {installPreview.update_diff.summary_lines.map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ul>
              </div>
            )}
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
              <button type="button" onClick={() => setInstallPreview(null)}>
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void handleConfirmInstall()}
                style={{ background: "#1ea7d8", color: "#fff", border: "none", padding: "8px 16px", borderRadius: 8 }}
              >
                Trust & Install
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
