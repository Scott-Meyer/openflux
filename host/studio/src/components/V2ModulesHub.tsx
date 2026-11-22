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

const CATEGORY_TABS: Array<{ id: ModuleCategory; label: string; subtitle: string }> = [
  { id: "effect", label: "Key Press Effects", subtitle: "Stackable — combine 3D water, analog depth glow, particles & classics" },
  { id: "widget", label: "Top Bar Widgets", subtitle: "Place modules into top-bar slots 0–3" },
  { id: "background", label: "Backgrounds", subtitle: "Live WGSL/GLSL shaders, images, or video textures" },
  { id: "companion", label: "Companions", subtitle: "Spatial modules that live in a slot and roam across physical keys" },
  { id: "host_service", label: "Host Services", subtitle: "Unrestricted host processes & local API integrations" },
];

export const V2ModulesHub: React.FC = () => {
  const [modules, setModules] = useState<V2ModuleRecord[]>([]);
  const [lists, setLists] = useState<CuratedListSubscription[]>([]);
  const [category, setCategory] = useState<ModuleCategory>("effect");
  const [selectedId, setSelectedId] = useState<string | null>(null);
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
      setLists(lRes.lists);
      if (typeof mRes.modules_rev === "number") {
        modulesRevRef.current = mRes.modules_rev;
      }
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

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
    const targetId = selectedId ?? modules.find((m) => m.manifest.category === category)?.id;
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
  }, [selectedId, category, modules]);

  const handleToggle = async (mod: V2ModuleRecord) => {
    try {
      await v2Client.updateModuleState(mod.id, { enabled: !mod.state.enabled });
      await refresh();
      scheduleKeyboardSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleSlotChange = async (mod: V2ModuleRecord, raw: string) => {
    try {
      const slot = raw === "" ? null : Number(raw);
      await v2Client.updateModuleState(mod.id, { slot });
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
      setCategory(newModCat);
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

  const filtered = modules.filter((m) => m.manifest.category === category);
  // Scope selection to the active category so switching tabs always shows settings for a module in this tab!
  const selected = filtered.find((m) => m.id === selectedId) ?? filtered[0] ?? null;

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

      {/* Friendly Category Bar (All backed by the same unified Module runtime) */}
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
        {CATEGORY_TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => setCategory(t.id)}
            style={{
              padding: "8px 14px",
              borderRadius: 8,
              border: category === t.id ? "1px solid #4de1ff" : "1px solid rgba(255,255,255,0.14)",
              background: category === t.id ? "rgba(77, 225, 255, 0.16)" : "rgba(20, 24, 34, 0.7)",
              color: "#fff",
              cursor: "pointer",
            }}
          >
            {t.label} ({modules.filter((m) => m.manifest.category === t.id).length})
          </button>
        ))}
      </div>

      {/* Module List + Inspector / Settings */}
      <div style={{ display: "grid", gridTemplateColumns: "1.2fr 1fr", gap: 16 }}>
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
                  {mod.manifest.slot_capable && (
                    <select
                      aria-label={`Slot for ${mod.manifest.name}`}
                      value={mod.state.slot ?? ""}
                      onChange={(e) => void handleSlotChange(mod, e.target.value)}
                    >
                      <option value="">No Slot</option>
                      <option value="0">Slot 0</option>
                      <option value="1">Slot 1</option>
                      <option value="2">Slot 2</option>
                      <option value="3">Slot 3</option>
                    </select>
                  )}
                  <button type="button" onClick={() => void handleToggle(mod)}>
                    {mod.state.enabled ? "Enabled" : "Disabled"}
                  </button>
                </div>
              </div>
              <div style={{ fontSize: 12, opacity: 0.78, marginTop: 6 }}>{mod.manifest.description}</div>
              {!mod.healthy && mod.last_error && (
                <div style={{ fontSize: 12, color: "#ff8c96", marginTop: 6 }}>Runtime Error: {mod.last_error}</div>
              )}
            </div>
          ))}
        </div>

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
                <div style={{ display: "flex", gap: 6 }}>
                  <button type="button" onClick={() => void handleFork(selected)}>
                    Fork to Local
                  </button>
                  <button type="button" onClick={() => void v2Client.reloadModule(selected.id)}>
                    Hot Reload
                  </button>
                </div>
              </div>

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
                        {field.kind === "color" ? (
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
