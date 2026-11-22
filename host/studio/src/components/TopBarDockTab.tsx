import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { type V2ModuleRecord, v2Client } from "../api/v2Client";

export interface TopBarDockTabProps {
  onModulesChanged?: (mods: V2ModuleRecord[]) => void;
}

export const TopBarDockTab: React.FC<TopBarDockTabProps> = ({ onModulesChanged }) => {
  const [modules, setModules] = useState<V2ModuleRecord[]>([]);
  const [previewPng, setPreviewPng] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inflightRef = useRef(false);
  const syncTimerRef = useRef<number | null>(null);

  const scheduleSync = useCallback(() => {
    if (syncTimerRef.current !== null) window.clearTimeout(syncTimerRef.current);
    syncTimerRef.current = window.setTimeout(() => {
      syncTimerRef.current = null;
      void v2Client.syncToKeyboard().catch(() => {
        // Ignore when unplugged
      });
    }, 260);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const res = await v2Client.listModules();
      setModules(res.modules);
      onModulesChanged?.(res.modules);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [onModulesChanged]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      if (!alive || inflightRef.current) return;
      inflightRef.current = true;
      try {
        const res = await v2Client.renderPreviewFrame(1, 16.667, 960, 540);
        if (alive) setPreviewPng(`data:image/png;base64,${res.png_base64}`);
      } catch {
        // ignore
      } finally {
        inflightRef.current = false;
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 220);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const dockMod = modules.find((m) => m.id === "dock-top-bar");
  const slotCount = Math.max(2, Math.min(4, Number(dockMod?.state.config.slot_count ?? 4) || 4));
  const dimming = Number(dockMod?.state.config.module_dimming ?? 28);
  const showDividers = dockMod?.state.config.show_dividers !== false;

  const slotCapable = modules.filter((m) => m.manifest.slot_capable);

  const occupantForSlot = (slotIdx: number) =>
    slotCapable.find((m) => m.state.enabled && m.state.slot === slotIdx) ?? null;

  const handleAssignSlot = async (slotIdx: number, nextModuleId: string) => {
    try {
      const prev = occupantForSlot(slotIdx);
      if (prev && prev.id !== nextModuleId) {
        await v2Client.updateModuleState(prev.id, { slot: null });
      }
      if (nextModuleId) {
        await v2Client.updateModuleState(nextModuleId, { enabled: true, slot: slotIdx });
        setNotice(`Assigned ${nextModuleId} to Slot ${slotIdx}`);
      } else if (prev) {
        setNotice(`Cleared Slot ${slotIdx}`);
      }
      await refresh();
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleDockSetting = async (key: string, value: unknown) => {
    if (!dockMod) return;
    setModules((prev) =>
      prev.map((m) =>
        m.id === dockMod.id
          ? { ...m, state: { ...m.state, config: { ...m.state.config, [key]: value } } }
          : m,
      ),
    );
    try {
      await v2Client.updateModuleState(dockMod.id, { config: { [key]: value } });
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleSpanChange = async (mod: V2ModuleRecord, span: number) => {
    setModules((prev) =>
      prev.map((m) => (m.id === mod.id ? { ...m, state: { ...m.state, slot_span: span } } : m)),
    );
    try {
      await v2Client.updateModuleState(mod.id, { slot_span: span });
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleWidgetSetting = async (mod: V2ModuleRecord, key: string, value: unknown) => {
    setModules((prev) =>
      prev.map((m) =>
        m.id === mod.id
          ? { ...m, state: { ...m.state, config: { ...m.state.config, [key]: value } } }
          : m,
      ),
    );
    try {
      await v2Client.updateModuleState(mod.id, { config: { [key]: value } });
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16, padding: 16 }}>
      <section
        style={{
          background: "rgba(14, 18, 28, 0.9)",
          border: "1px solid rgba(110, 180, 255, 0.25)",
          borderRadius: 12,
          padding: 14,
        }}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10 }}>
          <div>
            <strong>Top Bar Dock</strong>
            <span style={{ marginLeft: 10, fontSize: 12, opacity: 0.7 }}>
              Provided by module <code>dock-top-bar</code> — changes sync live to your keyboard
            </span>
          </div>
          <button
            type="button"
            style={{ background: "#1ea7d8", color: "#fff", border: "none", padding: "6px 12px", borderRadius: 6 }}
            onClick={() => void v2Client.syncToKeyboard().then(() => setNotice("Synced Top Bar to keyboard."))}
          >
            Sync to Keyboard
          </button>
        </div>

        {previewPng && (
          <div style={{ width: "100%", maxWidth: 960, margin: "0 auto" }}>
            <img
              src={previewPng}
              alt="Top Bar Live Preview"
              style={{
                width: "100%",
                aspectRatio: "16 / 9",
                borderRadius: 10,
                display: "block",
                border: "1px solid rgba(255,255,255,0.12)",
              }}
            />
          </div>
        )}
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

      {/* Visual Top Bar Slot Picker Grid */}
      <section
        style={{
          display: "grid",
          gridTemplateColumns: `repeat(${slotCount}, minmax(0, 1fr))`,
          gap: 12,
        }}
      >
        {Array.from({ length: slotCount }, (_, slotIdx) => {
          const occ = occupantForSlot(slotIdx);
          return (
            <div
              key={slotIdx}
              style={{
                padding: 14,
                borderRadius: 10,
                border: "1px solid rgba(255,255,255,0.14)",
                background: "rgba(18, 22, 32, 0.88)",
                display: "flex",
                flexDirection: "column",
                gap: 10,
              }}
            >
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <strong>Slot {slotIdx + 1}</strong>
                <span style={{ fontSize: 11, opacity: 0.6 }}>index {slotIdx}</span>
              </div>
              <select
                aria-label={`Module for Slot ${slotIdx + 1}`}
                value={occ?.id ?? ""}
                onChange={(e) => void handleAssignSlot(slotIdx, e.target.value)}
                style={{ padding: "7px 8px", borderRadius: 6 }}
              >
                <option value="">Empty</option>
                {slotCapable.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.manifest.name}
                  </option>
                ))}
              </select>

              {occ && (
                <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 4 }}>
                  <label style={{ display: "flex", alignItems: "center", justifyContent: "space-between", fontSize: 12 }}>
                    <span>Width Span</span>
                    <select
                      aria-label={`Width for ${occ.manifest.name}`}
                      value={String(occ.state.slot_span ?? 1)}
                      onChange={(e) => void handleSpanChange(occ, Number(e.target.value))}
                    >
                      <option value="0.5">½ Slot</option>
                      <option value="1">1 Slot</option>
                      <option value="2">2 Slots</option>
                      <option value="3">3 Slots</option>
                      <option value="4">4 Slots (Full Bar)</option>
                    </select>
                  </label>
                  <div style={{ fontSize: 11, opacity: 0.7 }}>{occ.manifest.description}</div>
                  {occ.manifest.settings.slice(0, 4).map((field) => {
                    const cur = occ.state.config[field.key] ?? field.default ?? "";
                    return (
                      <label key={field.key} style={{ display: "flex", flexDirection: "column", gap: 3, fontSize: 12 }}>
                        <span>{field.label}</span>
                        {field.kind === "boolean" ? (
                          <input
                            type="checkbox"
                            checked={Boolean(cur)}
                            onChange={(e) => void handleWidgetSetting(occ, field.key, e.target.checked)}
                          />
                        ) : field.kind === "color" ? (
                          <input
                            type="color"
                            value={String(cur || "#4de1ff")}
                            onChange={(e) => void handleWidgetSetting(occ, field.key, e.target.value)}
                          />
                        ) : (
                          <input
                            type={field.secret ? "password" : field.kind === "number" ? "number" : "text"}
                            value={String(cur)}
                            onChange={(e) =>
                              void handleWidgetSetting(
                                occ,
                                field.key,
                                field.kind === "number" ? Number(e.target.value) : e.target.value,
                              )
                            }
                          />
                        )}
                      </label>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </section>

      {/* Top Bar Dock Appearance Controls */}
      <section
        style={{
          padding: 14,
          borderRadius: 10,
          border: "1px solid rgba(255,255,255,0.12)",
          background: "rgba(18, 22, 32, 0.85)",
          display: "grid",
          gridTemplateColumns: "1fr 2fr 1fr",
          gap: 16,
          alignItems: "center",
        }}
      >
        <label style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 13 }}>
          <strong>Number of Slots</strong>
          <select
            value={String(slotCount)}
            onChange={(e) => void handleDockSetting("slot_count", e.target.value)}
          >
            <option value="4">4 Slots (Classic)</option>
            <option value="3">3 Wide Slots</option>
            <option value="2">2 Extra-Wide Slots</option>
          </select>
        </label>

        <label style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 13 }}>
          <strong>Top Bar Soft Vignette Fade: {dimming}%</strong>
          <input
            type="range"
            min={0}
            max={80}
            step={1}
            value={dimming}
            onChange={(e) => void handleDockSetting("module_dimming", Number(e.target.value))}
          />
        </label>

        <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
          <input
            type="checkbox"
            checked={showDividers}
            onChange={(e) => void handleDockSetting("show_dividers", e.target.checked)}
          />
          <strong>Show Slot Dividers</strong>
        </label>
      </section>
    </div>
  );
};
