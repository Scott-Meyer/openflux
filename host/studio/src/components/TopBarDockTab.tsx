import { useCallback, useEffect, useRef, useState } from "react";
import { v2Client, type V2ModuleRecord } from "../api/v2Client";

function getSupportedSpans(m: V2ModuleRecord): number[] {
  const s = m.manifest.top_bar?.supported_spans;
  return s && s.length > 0 ? s : [1.0];
}

function getDefaultSpan(m: V2ModuleRecord): number {
  return m.manifest.top_bar?.default_span ?? getSupportedSpans(m)[0] ?? 1.0;
}

function isSingleton(m: V2ModuleRecord): boolean {
  return m.manifest.top_bar?.singleton ?? true;
}

function effectivePos(m: V2ModuleRecord): number | null {
  if (typeof m.state.slot_pos === "number") return m.state.slot_pos;
  if (typeof m.state.slot === "number") return m.state.slot;
  return null;
}

function effectiveSpan(m: V2ModuleRecord): number {
  const allowed = getSupportedSpans(m);
  const raw = m.state.slot_span ?? getDefaultSpan(m);
  return allowed.includes(raw) ? raw : allowed[0] ?? 1.0;
}

function fmtSpan(span: number): string {
  if (span === 0.5) return "½";
  if (span === 1) return "1";
  return `${span}`;
}

interface PlacedItem {
  mod: V2ModuleRecord;
  startHalf: number; // integer 0..(totalHalf - 1)
  spanHalf: number;  // integer 1..totalHalf
}

type TrackSegment =
  | { kind: "widget"; item: PlacedItem }
  | { kind: "empty"; startHalf: number; spanHalf: number };

export function TopBarDockTab({
  onModulesChanged,
}: {
  onModulesChanged?: (mods: V2ModuleRecord[]) => void;
}) {
  const [modules, setModules] = useState<V2ModuleRecord[]>([]);
  const [previewSrc, setPreviewSrc] = useState<string>("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const syncTimerRef = useRef<number | null>(null);

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
    const timer = window.setInterval(() => void refresh(), 2500);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    let alive = true;
    let inFlight = false;
    const pullFrame = async () => {
      if (inFlight || !alive) return;
      inFlight = true;
      try {
        const res = await v2Client.renderPreviewFrame(1, 33, 960, 540);
        if (alive && res.png_base64) {
          setPreviewSrc(`data:image/png;base64,${res.png_base64}`);
        }
      } catch {
        // ignore transient errors
      } finally {
        inFlight = false;
      }
    };
    void pullFrame();
    const timer = window.setInterval(() => void pullFrame(), 400);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const scheduleSync = useCallback(() => {
    if (syncTimerRef.current !== null) window.clearTimeout(syncTimerRef.current);
    syncTimerRef.current = window.setTimeout(() => {
      syncTimerRef.current = null;
      void v2Client.syncToKeyboard().catch(() => {});
    }, 300);
  }, []);

  const dock =
    modules.find((m) => (m.manifest.provides ?? []).includes("top_bar_dock") && m.state.enabled) ??
    modules.find((m) => (m.manifest.provides ?? []).includes("top_bar_dock")) ??
    modules.find((m) => m.id === "dock-top-bar");
  const slotCount = Math.max(2, Math.min(4, Number(dock?.state.config.slot_count ?? 4) || 4));
  const totalHalf = slotCount * 2;
  const dimming = Number(dock?.state.config.module_dimming ?? 0);
  const showDividers = dock?.state.config.show_dividers !== false;

  // All widgets that explicitly support top-bar placement
  const topBarWidgets = modules
    .filter((m) => m.manifest.slot_capable && !(m.manifest.provides ?? []).includes("top_bar_dock"))
    .sort((a, b) => a.manifest.name.localeCompare(b.manifest.name));

  // Build collision-free occupied grid in half-slot units
  const placed: PlacedItem[] = [];
  const halfOwner: Array<string | null> = Array.from({ length: totalHalf }, () => null);
  for (const w of topBarWidgets) {
    if (!w.state.enabled) continue;
    const p = effectivePos(w);
    if (p === null) continue;
    const startHalf = Math.round(p * 2);
    if (startHalf < 0 || startHalf >= totalHalf) continue;
    const spanHalf = Math.min(totalHalf - startHalf, Math.max(1, Math.round(effectiveSpan(w) * 2)));
    let overlaps = false;
    for (let h = startHalf; h < startHalf + spanHalf; h++) {
      if (halfOwner[h] !== null) {
        overlaps = true;
        break;
      }
    }
    if (!overlaps) {
      for (let h = startHalf; h < startHalf + spanHalf; h++) halfOwner[h] = w.id;
      placed.push({ mod: w, startHalf, spanHalf });
    }
  }
  placed.sort((a, b) => a.startHalf - b.startHalf);

  // Build visual horizontal track segments (placed widgets + open gaps)
  const segments: TrackSegment[] = [];
  let cursor = 0;
  while (cursor < totalHalf) {
    const item = placed.find((p) => p.startHalf === cursor);
    if (item) {
      segments.push({ kind: "widget", item });
      cursor += item.spanHalf;
    } else {
      // Group contiguous free half-slots (split at 1-slot chunks if >=2 so user can drop into Slot 1, Slot 2, etc.)
      let freeLen = 0;
      while (cursor + freeLen < totalHalf && halfOwner[cursor + freeLen] === null) {
        freeLen += 1;
      }
      // If aligned on an integer slot and >= 2 half-slots free, show 1-slot empty tiles (while still knowing full contiguous space!)
      const step = cursor % 2 === 0 && freeLen >= 2 ? 2 : 1;
      segments.push({ kind: "empty", startHalf: cursor, spanHalf: step });
      cursor += step;
    }
  }

  const maxContiguousFrom = (startHalf: number, ignoreId?: string): number => {
    let len = 0;
    while (
      startHalf + len < totalHalf &&
      (halfOwner[startHalf + len] === null || halfOwner[startHalf + len] === ignoreId)
    ) {
      len += 1;
    }
    return len;
  };

  const handleDockSetting = async (key: string, value: unknown) => {
    if (!dock) return;
    setModules((prev) =>
      prev.map((m) =>
        m.id === dock.id
          ? { ...m, state: { ...m.state, config: { ...m.state.config, [key]: value } } }
          : m,
      ),
    );
    try {
      await v2Client.updateModuleState(dock.id, { config: { [key]: value } });
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handlePlaceWidget = async (modId: string, startHalf: number, maxAvailHalf: number) => {
    const mod = topBarWidgets.find((w) => w.id === modId);
    if (!mod) return;
    const supported = getSupportedSpans(mod);
    const def = getDefaultSpan(mod);
    // Pick default span if it fits, else largest supported span that fits!
    const fitting = supported.filter((s) => Math.round(s * 2) <= maxAvailHalf);
    if (fitting.length === 0) return;
    const chosenSpan = fitting.includes(def) ? def : fitting[fitting.length - 1];
    try {
      await v2Client.updateModuleState(mod.id, {
        enabled: true,
        slot_pos: startHalf / 2,
        slot_span: chosenSpan,
      });
      setSelectedId(mod.id);
      setNotice(`Placed ${mod.manifest.name} (${fmtSpan(chosenSpan)} slot)`);
      await refresh();
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleRemoveWidget = async (mod: V2ModuleRecord) => {
    try {
      await v2Client.updateModuleState(mod.id, { slot_pos: null, slot: null });
      if (selectedId === mod.id) setSelectedId(null);
      setNotice(`Removed ${mod.manifest.name} from Top Bar`);
      await refresh();
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleSpanChange = async (item: PlacedItem, newSpan: number) => {
    try {
      await v2Client.updateModuleState(item.mod.id, {
        slot_pos: item.startHalf / 2,
        slot_span: newSpan,
      });
      await refresh();
      scheduleSync();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleShift = async (item: PlacedItem, deltaHalf: number) => {
    const nextStart = item.startHalf + deltaHalf;
    if (nextStart < 0 || nextStart + item.spanHalf > totalHalf) return;
    try {
      await v2Client.updateModuleState(item.mod.id, {
        slot_pos: nextStart / 2,
        slot_span: item.spanHalf / 2,
      });
      await refresh();
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

  const selectedMod = placed.find((p) => p.mod.id === selectedId)?.mod ?? placed[0]?.mod ?? null;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: 10 }}>
        <div>
          <h2 style={{ margin: 0 }}>Top Bar Layout</h2>
          <p style={{ margin: "4px 0 0", opacity: 0.72, fontSize: 13 }}>
            Proportional half-slot dock — widgets declare which widths they support (½, 1, 2, 3, 4) and never overlap.
          </p>
        </div>
        <button
          type="button"
          onClick={() =>
            void v2Client
              .syncToKeyboard()
              .then((r) => setNotice(`Synced Top Bar to keyboard (${r.uploaded_files.length} files)`))
              .catch((e: unknown) => setError(String(e)))
          }
        >
          Sync to Keyboard
        </button>
      </div>

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

      {/* Live Preview */}
      {previewSrc && (
        <div
          style={{
            borderRadius: 12,
            overflow: "hidden",
            border: "1px solid rgba(255,255,255,0.12)",
            background: "#090d16",
          }}
        >
          <img src={previewSrc} alt="Live Top Bar preview" style={{ width: "100%", display: "block" }} />
        </div>
      )}

      {/* Dock Appearance Controls */}
      <section
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
          gap: 14,
          padding: 14,
          borderRadius: 12,
          border: "1px solid rgba(255,255,255,0.1)",
          background: "rgba(18, 22, 32, 0.7)",
        }}
      >
        <label style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 13 }}>
          <strong>Grid Slots</strong>
          <select
            value={String(slotCount)}
            onChange={(e) => void handleDockSetting("slot_count", e.target.value)}
          >
            <option value="4">4 Slots (8 half-slots)</option>
            <option value="3">3 Slots (6 half-slots)</option>
            <option value="2">2 Slots (4 half-slots)</option>
          </select>
        </label>

        <label style={{ display: "flex", flexDirection: "column", gap: 4, fontSize: 13 }}>
          <strong>Top Bar Tint Dimming: {dimming}%</strong>
          <input
            type="range"
            min={0}
            max={80}
            step={1}
            value={dimming}
            onChange={(e) => void handleDockSetting("module_dimming", Number(e.target.value))}
          />
        </label>

        <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13, alignSelf: "center" }}>
          <input
            type="checkbox"
            checked={showDividers}
            onChange={(e) => void handleDockSetting("show_dividers", e.target.checked)}
          />
          <strong>Show Smart Slot Dividers</strong>
        </label>
      </section>

      {/* Proportional Half-Slot Visual Top Bar Track */}
      <section style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: `repeat(${totalHalf}, minmax(0, 1fr))`,
            gap: 8,
          }}
        >
          {segments.map((seg) => {
            if (seg.kind === "widget") {
              const { item } = seg;
              const { mod } = item;
              const supported = getSupportedSpans(mod);
              const maxHalfHere = maxContiguousFrom(item.startHalf, mod.id);
              const canMoveLeft = item.startHalf > 0 && halfOwner[item.startHalf - 1] === null;
              const canMoveRight =
                item.startHalf + item.spanHalf < totalHalf &&
                halfOwner[item.startHalf + item.spanHalf] === null;
              const isSelected = selectedMod?.id === mod.id;

              return (
                <div
                  key={mod.id}
                  onClick={() => setSelectedId(mod.id)}
                  style={{
                    gridColumn: `span ${item.spanHalf}`,
                    padding: 12,
                    borderRadius: 12,
                    border: isSelected
                      ? "1px solid #4de1ff"
                      : "1px solid rgba(255,255,255,0.14)",
                    background: isSelected
                      ? "rgba(77, 225, 255, 0.10)"
                      : "rgba(22, 28, 42, 0.85)",
                    display: "flex",
                    flexDirection: "column",
                    gap: 8,
                    cursor: "pointer",
                    minWidth: 0,
                  }}
                >
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", gap: 6 }}>
                    <div style={{ minWidth: 0 }}>
                      <div style={{ fontWeight: 600, fontSize: 13, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                        {mod.manifest.name}
                      </div>
                      <div style={{ fontSize: 11, opacity: 0.65 }}>
                        {isSingleton(mod) ? "Singleton" : "Multi"} · {fmtSpan(item.spanHalf / 2)} slot
                      </div>
                    </div>
                    <button
                      type="button"
                      title="Remove from Top Bar"
                      onClick={(e) => {
                        e.stopPropagation();
                        void handleRemoveWidget(mod);
                      }}
                      style={{ padding: "2px 7px", fontSize: 11 }}
                    >
                      ✕
                    </button>
                  </div>

                  {/* Supported Width Span Pills (ONLY spans declared by this module!) */}
                  <div style={{ display: "flex", gap: 4, flexWrap: "wrap", alignItems: "center" }} onClick={(e) => e.stopPropagation()}>
                    {supported.map((sp) => {
                      const spHalf = Math.round(sp * 2);
                      const active = spHalf === item.spanHalf;
                      const fits = spHalf <= maxHalfHere;
                      return (
                        <button
                          key={sp}
                          type="button"
                          disabled={!fits && !active}
                          title={fits ? `Resize to ${fmtSpan(sp)} slot` : "Not enough open space to the right"}
                          onClick={() => void handleSpanChange(item, sp)}
                          style={{
                            fontSize: 11,
                            padding: "2px 7px",
                            borderRadius: 6,
                            border: active ? "1px solid #4de1ff" : "1px solid rgba(255,255,255,0.16)",
                            background: active ? "rgba(77, 225, 255, 0.22)" : "rgba(255,255,255,0.04)",
                            color: fits || active ? "#fff" : "rgba(255,255,255,0.3)",
                            cursor: fits || active ? "pointer" : "not-allowed",
                          }}
                        >
                          {fmtSpan(sp)}×
                        </button>
                      );
                    })}
                    <div style={{ marginLeft: "auto", display: "flex", gap: 3 }}>
                      <button
                        type="button"
                        disabled={!canMoveLeft}
                        title="Nudge left by ½ slot"
                        onClick={() => void handleShift(item, -1)}
                        style={{ fontSize: 11, padding: "2px 6px", opacity: canMoveLeft ? 1 : 0.3 }}
                      >
                        ←
                      </button>
                      <button
                        type="button"
                        disabled={!canMoveRight}
                        title="Nudge right by ½ slot"
                        onClick={() => void handleShift(item, 1)}
                        style={{ fontSize: 11, padding: "2px 6px", opacity: canMoveRight ? 1 : 0.3 }}
                      >
                        →
                      </button>
                    </div>
                  </div>
                </div>
              );
            }

            // Empty open space segment — only offer widgets whose minimum span fits!
            const maxAvailHalf = maxContiguousFrom(seg.startHalf);
            const availableUnplaced = topBarWidgets.filter((w) => {
              const placedHere = placed.some((p) => p.mod.id === w.id);
              const minHalf = Math.min(...getSupportedSpans(w).map((s) => Math.round(s * 2)));
              return !placedHere && minHalf <= maxAvailHalf;
            });
            const movableSingletons = topBarWidgets.filter((w) => {
              const placedHere = placed.some((p) => p.mod.id === w.id);
              const minHalf = Math.min(...getSupportedSpans(w).map((s) => Math.round(s * 2)));
              return placedHere && minHalf <= maxAvailHalf;
            });

            return (
              <div
                key={`empty-${seg.startHalf}`}
                style={{
                  gridColumn: `span ${seg.spanHalf}`,
                  padding: 12,
                  borderRadius: 12,
                  border: "1px dashed rgba(255,255,255,0.18)",
                  background: "rgba(14, 18, 28, 0.45)",
                  display: "flex",
                  flexDirection: "column",
                  justifyContent: "center",
                  gap: 6,
                  minWidth: 0,
                }}
              >
                <div style={{ fontSize: 11, opacity: 0.65 }}>
                  Open ({fmtSpan(seg.spanHalf / 2)} slot)
                </div>
                <select
                  aria-label={`Add widget at position ${seg.startHalf / 2}`}
                  value=""
                  onChange={(e) => {
                    if (e.target.value) void handlePlaceWidget(e.target.value, seg.startHalf, maxAvailHalf);
                  }}
                  style={{ width: "100%", fontSize: 12 }}
                >
                  <option value="">+ Add widget…</option>
                  {availableUnplaced.length > 0 && (
                    <optgroup label="Available">
                      {availableUnplaced.map((w) => (
                        <option key={w.id} value={w.id}>
                          {w.manifest.name} ({getSupportedSpans(w).map(fmtSpan).join("/")})
                        </option>
                      ))}
                    </optgroup>
                  )}
                  {movableSingletons.length > 0 && (
                    <optgroup label="Move from another slot (Singleton)">
                      {movableSingletons.map((w) => (
                        <option key={w.id} value={w.id}>
                          Move {w.manifest.name} here
                        </option>
                      ))}
                    </optgroup>
                  )}
                </select>
              </div>
            );
          })}
        </div>
      </section>

      {/* Selected Widget Quick Settings */}
      {selectedMod && (
        <section
          style={{
            padding: 14,
            borderRadius: 12,
            border: "1px solid rgba(255,255,255,0.1)",
            background: "rgba(18, 22, 32, 0.72)",
            display: "flex",
            flexDirection: "column",
            gap: 10,
          }}
        >
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <div>
              <strong>{selectedMod.manifest.name}</strong>
              <span style={{ fontSize: 12, opacity: 0.65, marginLeft: 8 }}>
                Supports: {getSupportedSpans(selectedMod).map((s) => `${fmtSpan(s)} slot`).join(", ")} ·{" "}
                {isSingleton(selectedMod) ? "Singleton widget" : "Multi-instance"}
              </span>
            </div>
          </div>
          <div style={{ fontSize: 12, opacity: 0.78 }}>{selectedMod.manifest.description}</div>

          {selectedMod.manifest.settings.length > 0 ? (
            <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))", gap: 10 }}>
              {selectedMod.manifest.settings.map((field) => {
                const cur = selectedMod.state.config[field.key] ?? field.default;
                if (field.kind === "boolean") {
                  return (
                    <label key={field.key} style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 13 }}>
                      <input
                        type="checkbox"
                        checked={Boolean(cur)}
                        onChange={(e) => void handleWidgetSetting(selectedMod, field.key, e.target.checked)}
                      />
                      <span>{field.label}</span>
                    </label>
                  );
                }
                if (field.kind === "string") {
                  return (
                    <label key={field.key} style={{ display: "flex", flexDirection: "column", gap: 3, fontSize: 12 }}>
                      <span>{field.label}</span>
                      <input
                        type={field.secret ? "password" : "text"}
                        value={String(cur ?? "")}
                        onChange={(e) => void handleWidgetSetting(selectedMod, field.key, e.target.value)}
                      />
                    </label>
                  );
                }
                if (field.kind === "color") {
                  return (
                    <label key={field.key} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12 }}>
                      <span>{field.label}</span>
                      <input
                        type="color"
                        value={String(cur || "#58f29c")}
                        onChange={(e) => void handleWidgetSetting(selectedMod, field.key, e.target.value)}
                      />
                    </label>
                  );
                }
                if (field.kind === "number") {
                  return (
                    <label key={field.key} style={{ display: "flex", flexDirection: "column", gap: 3, fontSize: 12 }}>
                      <span>
                        {field.label}: {Number(cur ?? 0)}
                      </span>
                      <input
                        type="range"
                        min={field.min ?? 0}
                        max={field.max ?? 100}
                        value={Number(cur ?? 0)}
                        onChange={(e) => void handleWidgetSetting(selectedMod, field.key, Number(e.target.value))}
                      />
                    </label>
                  );
                }
                return null;
              })}
            </div>
          ) : (
            <div style={{ fontSize: 12, opacity: 0.6 }}>No additional settings for this widget.</div>
          )}
        </section>
      )}
    </div>
  );
}
