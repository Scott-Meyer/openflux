import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DaemonClient } from "./api/daemonClient";
import { createDaemonClient } from "./api/daemonClient";
import { type V2ModuleRecord, v2Client } from "./api/v2Client";
import { ActuationPanel } from "./components/ActuationPanel";
import { DevicePanel } from "./components/DevicePanel";
import { HomeView } from "./components/HomeView";
import { KeycapsTab } from "./components/KeycapsTab";
import { type SurfaceKeyGeom } from "./components/KeyboardSurface";
import { ModuleStoreView } from "./components/ModuleStoreView";
import { ModuleView } from "./components/ModuleView";
import { QmkKeymapView } from "./components/QmkKeymapView";
import { ShortcutsProfilesPanel } from "./components/ShortcutsProfilesPanel";
import { TopBarDockTab } from "./components/TopBarDockTab";
import type { AssetCatalog, DeviceStatus, OpenFluxConfig, PreviewKeyRect } from "./types/openflux";
import { isKeyboardConnected, PLACEHOLDER_CONFIG } from "./types/openflux";

export type StudioView =
  | { kind: "home" }
  | { kind: "module"; moduleId: string; focusKey?: number | null }
  | { kind: "actuation" }
  | { kind: "store" }
  | { kind: "device" };

interface AppProps {
  client?: DaemonClient;
  initialConfig?: OpenFluxConfig;
  platform?: string;
}

const SIDEBAR_ORDER: Record<string, number> = {
  "keys-keymap": 1,
  "keys-custom": 2,
  "keys-shortcuts": 3,
  "dock-top-bar": 4,
  "bg-media": 10,
  "bg-aurora-shader": 11,
  "bg-stream": 12,
  "fx-water-ripple-3d": 20,
  "fx-classic-ripple": 21,
  "fx-classic-rings": 22,
  "fx-classic-glow": 23,
  "analog-gamepad": 30,
  "widget-prusa-connect": 40,
  "widget-clock": 41,
  "widget-weather": 42,
  "widget-system-load": 43,
  "widget-media": 44,
  "widget-pomodoro": 45,
};

function moduleGlyph(m: V2ModuleRecord): string {
  if (m.id === "keys-keymap") return "⌨";
  if (m.id === "keys-custom" || m.id === "keys-renderer") return "◈";
  if (m.id === "keys-shortcuts") return "⌘";
  if (m.id === "dock-top-bar") return "▭";
  if (m.id === "analog-gamepad") return "⊕";
  if (m.id === "widget-prusa-connect") return "▣";
  if (m.manifest.category === "background") return "≈";
  if (m.manifest.category === "effect") return "✦";
  if (m.manifest.category === "widget") return "◫";
  return "●";
}

function sidebarLabel(m: V2ModuleRecord): string {
  if (m.id === "keys-keymap") return "QMK Keymap";
  if (m.id === "keys-custom") return "Keycap Visuals";
  if (m.id === "dock-top-bar") return "Top Bar Dock";
  if (m.id === "keys-shortcuts") return "Shortcuts";
  if (m.id === "analog-gamepad") return "Analog Gamepad";
  if (m.id === "widget-prusa-connect") return "Prusa Printer";
  return m.manifest.ui?.top_tab_label || m.manifest.name;
}

export const App: React.FC<AppProps> = ({ client: injectedClient, initialConfig, platform }) => {
  const [client] = useState<DaemonClient>(() => injectedClient ?? createDaemonClient());
  const [view, setView] = useState<StudioView>({ kind: "home" });
  const [modules, setModules] = useState<V2ModuleRecord[]>([]);
  const [geomKeys, setGeomKeys] = useState<SurfaceKeyGeom[]>([]);
  const [v2UsbProtocol, setV2UsbProtocol] = useState<string>("offline");
  const [config, setConfig] = useState<OpenFluxConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  const [status, setStatus] = useState<DeviceStatus | null>(null);
  const [catalog, setCatalog] = useState<AssetCatalog | null>(null);
  const [previewRects, setPreviewRects] = useState<PreviewKeyRect[]>([]);
  const [selectedKeyIndex, setSelectedKeyIndex] = useState<number | null>(0);
  const [syncing, setSyncing] = useState(false);
  const [toastMsg, setToastMsg] = useState<string | null>(null);
  const [previewTick, setPreviewTick] = useState(0);
  const configRef = useRef<OpenFluxConfig>(config);
  configRef.current = config;

  const refreshModules = useCallback(async () => {
    try {
      const res = await v2Client.listModules();
      setModules(res.modules);
      setPreviewTick((t) => t + 1);
    } catch {
      // daemon may still be starting
    }
  }, []);

  const loadGeometry = useCallback(async () => {
    try {
      const res = await v2Client.getBusRetained("keyboard/geometry");
      const g = res.messages[0]?.payload as { keys?: SurfaceKeyGeom[] } | undefined;
      if (Array.isArray(g?.keys) && g.keys.length > 0) {
        setGeomKeys(g.keys);
      }
    } catch {
      // ignore
    }
  }, []);

  useEffect(() => {
    void refreshModules();
    void loadGeometry();
    void client
      .getConfig()
      .then((c) => setConfig(c))
      .catch(() => {});
    void client
      .getAssetsCatalog()
      .then((c) => setCatalog(c))
      .catch(() => {});
    void client
      .getPreviewRects("ansi")
      .then((r) => setPreviewRects(r))
      .catch(() => {});
  }, [client, refreshModules, loadGeometry]);

  useEffect(() => {
    let alive = true;
    const poll = () => {
      void v2Client
        .getStatus()
        .then((s) => {
          if (alive && s.usb_protocol) setV2UsbProtocol(s.usb_protocol);
        })
        .catch(() => {});
      void client
        .getStatus()
        .then((s) => {
          if (alive) setStatus(s);
        })
        .catch(() => {});
    };
    poll();
    const id = window.setInterval(poll, 3000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [client]);

  const keyboardConnected = v2UsbProtocol === "ofv2" || isKeyboardConnected(status);

  const pushConfigPatch = useCallback(
    async (patch: Partial<OpenFluxConfig>) => {
      const next = { ...configRef.current, ...patch };
      setConfig(next);
      try {
        await client.updateConfig(patch);
      } catch (err) {
        setToastMsg(err instanceof Error ? err.message : String(err));
      }
    },
    [client],
  );

  const handleSyncToKeyboard = async () => {
    setSyncing(true);
    setToastMsg(null);
    try {
      const res = await v2Client.syncToKeyboard();
      setToastMsg(`Synced ${res.uploaded_files.length} files to keyboard.`);
      window.setTimeout(() => setToastMsg(null), 3500);
    } catch (err) {
      setToastMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSyncing(false);
    }
  };

  // Sidebar shows ONLY the user's currently active modules (calm, thin-shell 5a/5b design)
  const sidebarModules = useMemo(() => {
    return modules
      .filter((m) => m.state.enabled && m.id !== "keys-renderer")
      .sort(
        (a, b) =>
          (SIDEBAR_ORDER[a.id] ?? 100) - (SIDEBAR_ORDER[b.id] ?? 100) ||
          a.manifest.name.localeCompare(b.manifest.name),
      );
  }, [modules]);

  const activeMod = useMemo(() => {
    if (view.kind !== "module") return null;
    return modules.find((m) => m.id === view.moduleId) ?? null;
  }, [view, modules]);

  const openModule = (moduleId: string, focusKey?: number) => {
    setView({ kind: "module", moduleId, focusKey: focusKey ?? null });
  };

  return (
    <div className="kg-shell">
      {/* Ultra-Calm Narrow Thin-Shell Left Sidebar (5a / 5b) */}
      <aside className="kg-sidebar" aria-label="KeyGlass Modules">
        <button
          type="button"
          className={`kg-brand-pill ${view.kind === "home" ? "is-active" : ""}`}
          onClick={() => setView({ kind: "home" })}
          title="Return to live keyboard overview"
        >
          <span className="kg-brand-icon">◇</span>
          <span className="kg-brand-name">KeyGlass</span>
          <span className={`kg-conn-dot ${keyboardConnected ? "is-on" : ""}`} />
          <span className={`kg-conn-text ${keyboardConnected ? "is-on" : ""}`}>
            {keyboardConnected ? "Connected" : "Offline"}
          </span>
        </button>

        <div className="kg-sidebar-section-label">Modules</div>
        <nav className="kg-module-nav">
          {sidebarModules.map((m) => {
            const isSel = view.kind === "module" && view.moduleId === m.id;
            return (
              <button
                key={m.id}
                type="button"
                className={`kg-nav-item ${isSel ? "is-active" : ""}`}
                onClick={() => openModule(m.id)}
              >
                <span className="kg-nav-glyph">{moduleGlyph(m)}</span>
                <span className="kg-nav-label">{sidebarLabel(m)}</span>
                {!m.healthy && <span className="kg-err-dot" title={m.last_error ?? "Module error"} />}
              </button>
            );
          })}

          <button
            type="button"
            className={`kg-nav-item kg-nav-add ${view.kind === "store" ? "is-active" : ""}`}
            onClick={() => setView({ kind: "store" })}
          >
            <span className="kg-nav-glyph">+</span>
            <span className="kg-nav-label">Add Module</span>
          </button>
        </nav>

        <div className="kg-sidebar-footer">
          <button
            type="button"
            className="kg-nav-item"
            disabled={syncing}
            onClick={() => void handleSyncToKeyboard()}
          >
            <span className="kg-nav-glyph">↻</span>
            <span className="kg-nav-label">{syncing ? "Syncing…" : "Sync to Keyboard"}</span>
          </button>
          <button
            type="button"
            className={`kg-nav-item ${view.kind === "actuation" ? "is-active" : ""}`}
            onClick={() => setView({ kind: "actuation" })}
          >
            <span className="kg-nav-glyph">∿</span>
            <span className="kg-nav-label">Hall Actuation</span>
          </button>
          <button
            type="button"
            className={`kg-nav-item ${view.kind === "device" ? "is-active" : ""}`}
            onClick={() => setView({ kind: "device" })}
          >
            <span className="kg-nav-glyph">⚙</span>
            <span className="kg-nav-label">Device &amp; About</span>
          </button>
        </div>
      </aside>

      {/* Expansive, Calm Single-Focus Main Canvas */}
      <main className="kg-canvas">
        {toastMsg && <div className="kg-toast-banner">{toastMsg}</div>}

        {view.kind === "home" && (
          <HomeView
            keys={geomKeys}
            modules={modules}
            onOpenModule={openModule}
            onOpenStore={() => setView({ kind: "store" })}
            previewTick={previewTick}
          />
        )}

        {view.kind === "module" && view.moduleId === "keys-keymap" && (
          <QmkKeymapView
            keys={geomKeys}
            initialKey={view.focusKey}
            onSynced={() => setPreviewTick((t) => t + 1)}
          />
        )}

        {view.kind === "module" && view.moduleId === "keys-custom" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Keycap Visuals</h1>
                <span className="kg-pill-subtle">Module · Overrides</span>
              </div>
            </div>
            <KeycapsTab pickFile={(title) => client.pickFile(title)} />
          </div>
        )}

        {view.kind === "module" && view.moduleId === "dock-top-bar" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Top Bar Dock</h1>
                <span className="kg-pill-subtle">Module · Layout</span>
              </div>
            </div>
            <TopBarDockTab onModulesChanged={setModules} />
          </div>
        )}

        {view.kind === "module" && view.moduleId === "keys-shortcuts" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Shortcuts</h1>
                <span className="kg-pill-subtle">Module · App &amp; OS Overlays</span>
              </div>
            </div>
            <ShortcutsProfilesPanel
              client={client}
              catalog={catalog}
              platform={platform}
              config={config}
              activeLayer={0}
              selectedKeyIndex={selectedKeyIndex}
              keymapBlocked={false}
              rules={config.appRules}
              onChangeRules={(appRules) => void pushConfigPatch({ appRules })}
              onChangeKeymap={(keymap) => void pushConfigPatch({ keymap })}
              onChangeAppearance={(theme) =>
                void pushConfigPatch({
                  appearance: { ...configRef.current.appearance, defaultTheme: theme },
                })
              }
              onImportBundle={(filePath) => client.importBundle(filePath)}
              onExportBundle={(outPath) => client.exportBundle(outPath)}
            />
          </div>
        )}

        {view.kind === "module" &&
          activeMod &&
          !["keys-keymap", "keys-custom", "dock-top-bar", "keys-shortcuts"].includes(view.moduleId) && (
            <ModuleView
              mod={activeMod}
              keys={geomKeys}
              pickFile={(title) => client.pickFile(title)}
              onUpdated={refreshModules}
              onOpenKeymap={() => openModule("keys-keymap")}
            />
          )}

        {view.kind === "store" && (
          <ModuleStoreView
            modules={modules}
            onUpdated={refreshModules}
            onOpenModule={(id) => openModule(id)}
          />
        )}

        {view.kind === "actuation" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Hall Actuation</h1>
                <span className="kg-pill-subtle">Hardware · Magnetic Travel</span>
              </div>
            </div>
            <ActuationPanel
              config={config}
              previewRects={previewRects}
              selectedKeyIndex={selectedKeyIndex}
              keymapBlocked={false}
              onSelectKey={setSelectedKeyIndex}
              onChangeActuation={(actuationModes) => void pushConfigPatch({ actuationModes })}
              onApplyConfigEdit={(patch) => void pushConfigPatch(patch)}
            />
          </div>
        )}

        {view.kind === "device" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Device &amp; About</h1>
                <span className="kg-pill-subtle">KeyGlass · Hardware &amp; Optical Calibration</span>
              </div>
            </div>
            <DevicePanel
              client={client}
              platform={platform}
              status={status}
              settings={config.settings}
              onChangeSettings={(settings) => void pushConfigPatch({ settings })}
              onRestoreDefaults={() => void client.getDefaultConfig().then((d) => setConfig(d))}
            />
          </div>
        )}
      </main>
    </div>
  );
};

export default App;
