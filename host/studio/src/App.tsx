import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DaemonClient } from "./api/daemonClient";
import { createDaemonClient } from "./api/daemonClient";
import { type ActiveProfileState, type V2ModuleRecord, v2Client } from "./api/v2Client";
import { ActuationPanel } from "./components/ActuationPanel";
import { DevicePanel } from "./components/DevicePanel";
import { HomeView } from "./components/HomeView";
import { KeycapsTab } from "./components/KeycapsTab";
import { type SurfaceKeyGeom } from "./components/KeyboardSurface";
import { ModuleStoreView } from "./components/ModuleStoreView";
import { ModuleView } from "./components/ModuleView";
import { ProfilesView } from "./components/ProfilesView";
import { QmkKeymapView } from "./components/QmkKeymapView";
import { ShortcutsProfilesPanel } from "./components/ShortcutsProfilesPanel";
import { StreamDeckView } from "./components/StreamDeckView";
import { TopBarDockTab } from "./components/TopBarDockTab";
import type { AssetCatalog, DeviceStatus, KeyglassConfig, PreviewKeyRect } from "./types/keyglass";
import { isKeyboardConnected, PLACEHOLDER_CONFIG } from "./types/keyglass";

export type StudioView =
  | { kind: "home" }
  | { kind: "module"; moduleId: string; focusKey?: number | null; focusProfileId?: string | null }
  | { kind: "actuation" }
  | { kind: "store" }
  | { kind: "device" };

interface AppProps {
  client?: DaemonClient;
  initialConfig?: KeyglassConfig;
  platform?: string;
}

const SIDEBAR_ORDER: Record<string, number> = {
  "profiles": 0,
  "keys-keymap": 1,
  "keys-custom": 2,
  "keys-shortcuts": 3,
  "streamdeck-bridge": 4,
  "dock-top-bar": 5,
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

const ModuleIcon: React.FC<{ mod: V2ModuleRecord }> = ({ mod }) => {
  const props = {
    width: 15,
    height: 15,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };
  if (mod.id === "keys-keymap") {
    return (
      <svg {...props}>
        <rect x="2" y="5" width="20" height="14" rx="2" />
        <path d="M6 9h.01M10 9h.01M14 9h.01M18 9h.01M7 13h10" />
      </svg>
    );
  }
  if (mod.id === "keys-custom" || mod.id === "keys-renderer") {
    return (
      <svg {...props}>
        <rect x="3" y="3" width="18" height="18" rx="3" />
        <circle cx="8.5" cy="8.5" r="1.5" />
        <path d="m21 15-5-5L5 21" />
      </svg>
    );
  }
  if (mod.id === "keys-shortcuts") {
    return (
      <svg {...props}>
        <path d="M18 3a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3 3 3 0 0 0 3-3 3 3 0 0 0-3-3H6a3 3 0 0 0-3 3 3 3 0 0 0 3 3 3 3 0 0 0 3-3V6a3 3 0 0 0-3-3 3 3 0 0 0-3 3 3 3 0 0 0 3 3h12a3 3 0 0 0 3-3 3 3 0 0 0-3-3z" />
      </svg>
    );
  }
  if (mod.id === "dock-top-bar") {
    return (
      <svg {...props}>
        <rect x="3" y="4" width="18" height="16" rx="2" />
        <path d="M3 10h18M9 4v6M15 4v6" />
      </svg>
    );
  }
  if (mod.id === "analog-gamepad") {
    return (
      <svg {...props}>
        <line x1="6" y1="12" x2="10" y2="12" />
        <line x1="8" y1="10" x2="8" y2="14" />
        <circle cx="15" cy="13" r="0.8" fill="currentColor" />
        <circle cx="17" cy="11" r="0.8" fill="currentColor" />
        <rect x="2" y="6" width="20" height="12" rx="4" />
      </svg>
    );
  }
  if (mod.id === "widget-prusa-connect") {
    return (
      <svg {...props}>
        <path d="M6 9V3h12v6M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2" />
        <rect x="6" y="14" width="12" height="7" rx="1" />
      </svg>
    );
  }
  if (mod.id === "widget-clock") {
    return (
      <svg {...props}>
        <circle cx="12" cy="12" r="9" />
        <path d="M12 7v5l3 2" />
      </svg>
    );
  }
  if (mod.id === "widget-weather") {
    return (
      <svg {...props}>
        <path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z" />
      </svg>
    );
  }
  if (mod.id === "widget-system-load") {
    return (
      <svg {...props}>
        <path d="M3 12h4l3-7 4 14 3-7h4" />
      </svg>
    );
  }
  if (mod.id === "widget-media") {
    return (
      <svg {...props}>
        <path d="M9 18V5l12-2v13" />
        <circle cx="6" cy="18" r="3" />
        <circle cx="18" cy="16" r="3" />
      </svg>
    );
  }
  if (mod.id === "host-volume-osd" || mod.id === "ui-osd-toast") {
    return (
      <svg {...props}>
        <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
        <path d="M15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14" />
      </svg>
    );
  }
  if (mod.manifest.category === "background") {
    return (
      <svg {...props}>
        <path d="M2 12c2-3 4-3 6 0s4 3 6 0 4-3 6 0" />
        <path d="M2 17c2-3 4-3 6 0s4 3 6 0 4-3 6 0" />
      </svg>
    );
  }
  if (mod.manifest.category === "effect") {
    return (
      <svg {...props}>
        <circle cx="12" cy="12" r="3" />
        <circle cx="12" cy="12" r="8" strokeDasharray="3 3" />
      </svg>
    );
  }
  return (
    <svg {...props}>
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <circle cx="12" cy="12" r="2.5" />
    </svg>
  );
};

function sidebarLabel(m: V2ModuleRecord): string {
  const clean: Record<string, string> = {
    "profiles": "Profiles",
    "keys-keymap": "QMK Keymap",
    "keys-custom": "Custom Legends",
    "dock-top-bar": "Top Bar & Attachments",
    "keys-shortcuts": "Modifier Legends",
    "streamdeck-bridge": "Stream Deck",
    "analog-gamepad": "Analog Gamepad",
    "widget-prusa-connect": "Prusa Printer",
    "widget-clock": "Clock",
    "widget-weather": "Weather",
    "widget-system-load": "System Load",
    "widget-media": "Now Playing",
    "widget-pomodoro": "Pomodoro",
    "fx-key-particles": "Key Sparks",
    "bg-media": "Background",
    "bg-aurora-shader": "Aurora Shader",
    "bg-stream": "Live Stream",
    "fx-classic-ripple": "Ripple Effect",
    "fx-water-ripple-3d": "Water Surface",
    "host-volume-osd": "Volume Watcher",
    "ui-osd-toast": "Notifications",
  };
  return clean[m.id] || m.manifest.ui?.top_tab_label || m.manifest.name;
}

export const App: React.FC<AppProps> = ({ client: injectedClient, initialConfig, platform }) => {
  const [client] = useState<DaemonClient>(() => injectedClient ?? createDaemonClient());
  const [view, setView] = useState<StudioView>({ kind: "home" });
  const [modules, setModules] = useState<V2ModuleRecord[]>([]);
  const [geomKeys, setGeomKeys] = useState<SurfaceKeyGeom[]>([]);
  const [v2UsbProtocol, setV2UsbProtocol] = useState<string>("offline");
  const [config, setConfig] = useState<KeyglassConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  const [status, setStatus] = useState<DeviceStatus | null>(null);
  const [catalog, setCatalog] = useState<AssetCatalog | null>(null);
  const [previewRects, setPreviewRects] = useState<PreviewKeyRect[]>([]);
  const [selectedKeyIndex, setSelectedKeyIndex] = useState<number | null>(0);
  const [syncing, setSyncing] = useState(false);
  const [syncConflict, setSyncConflict] = useState<{
    remoteHost: string;
    enabledCount: number;
    bgMedia?: string | null;
  } | null>(null);
  const [toastMsg, setToastMsg] = useState<string | null>(null);
  const [previewTick, setPreviewTick] = useState(0);
  const [activeProfile, setActiveProfile] = useState<ActiveProfileState | null>(null);
  const [profileMenuOpen, setProfileMenuOpen] = useState(false);
  const [htmlSdPlugins, setHtmlSdPlugins] = useState<Array<{ id: string; url: string }>>([]);
  const configRef = useRef<KeyglassConfig>(config);
  const modulesRevRef = useRef<number | null>(null);
  configRef.current = config;

  const refreshActiveProfile = useCallback(async () => {
    try {
      const res = await v2Client.getProfiles();
      if (res?.active) setActiveProfile(res.active);
    } catch {
      // daemon may still be starting
    }
  }, []);

  useEffect(() => {
    void refreshActiveProfile();
    const id = window.setInterval(() => void refreshActiveProfile(), 3000);
    return () => window.clearInterval(id);
  }, [refreshActiveProfile]);

  // Keep any imported HTML/JS `.sdPlugin` runtimes (like Task Timer's `index.html`) alive inside
  // hidden WebView iframes so they register on `sd_host.py`'s WebSocket server & update keycaps live!
  useEffect(() => {
    let alive = true;
    const syncHtmlPlugins = async () => {
      try {
        const sd = await v2Client.getStreamDeckState();
        const wsPort = sd.ws_port ?? 0;
        if (!alive || wsPort <= 0) return;
        const needed = new Set(sd.buttons.map((b) => b.plugin_id));
        const htmlPlugs = sd.plugins.filter(
          (p) =>
            needed.has(p.id) &&
            typeof p.code_path === "string" &&
            /\.(html?|HTML?)$/.test(p.code_path),
        );
        const urls = await Promise.all(
          htmlPlugs.map(async (p) => ({
            id: `${p.id}:${wsPort}`,
            url: await v2Client.getStreamDeckPluginUrl(p.id, p.code_path!, wsPort),
          })),
        );
        if (alive) {
          setHtmlSdPlugins((prev) => {
            const prevSig = prev.map((x) => x.id).join(",");
            const nextSig = urls.map((x) => x.id).join(",");
            return prevSig === nextSig ? prev : urls;
          });
        }
      } catch {
        // ignore when streamdeck-bridge is disabled
      }
    };
    void syncHtmlPlugins();
    const id = window.setInterval(() => void syncHtmlPlugins(), 4000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);

  const refreshModules = useCallback(async () => {
    try {
      const res = await v2Client.listModules();
      modulesRevRef.current = res.modules_rev;
      setModules(res.modules);
      setPreviewTick((t) => t + 1);
    } catch {
      // daemon may still be starting
    }
  }, []);

  const loadGeometry = useCallback(async () => {
    try {
      const res = await v2Client.getBusRetained("keyboard/geometry");
      const g = res.messages[0]?.payload as {
        keys?: Array<{
          key_index?: number;
          index?: number;
          row: number;
          col: number;
          center_x: number;
          center_y: number;
          rect: { x: number; y: number; w: number; h: number };
        }>;
      } | undefined;
      if (Array.isArray(g?.keys) && g.keys.length > 0) {
        // Normalize Rust `key_index` field -> `index` so clicking selects ONLY the single clicked key!
        setGeomKeys(
          g.keys.map((k, idx) => ({
            index: typeof k.key_index === "number" ? k.key_index : typeof k.index === "number" ? k.index : idx,
            row: k.row,
            col: k.col,
            center_x: k.center_x,
            center_y: k.center_y,
            rect: k.rect,
          })),
        );
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
    if (geomKeys.length > 0) return;
    let alive = true;
    const retry = () => {
      if (!alive) return;
      void loadGeometry();
    };
    const id = window.setInterval(retry, 1500);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [geomKeys.length, loadGeometry]);

  useEffect(() => {
    let alive = true;
    let daemonPid: number | undefined;
    const poll = () => {
      void v2Client
        .getStatus()
        .then((s) => {
          if (!alive) return;
          if (s.usb_protocol) setV2UsbProtocol(s.usb_protocol);
          // CLI/API edits change the same daemon state as Studio's own controls.
          // A restarted daemon can reuse a revision number, so check its PID too.
          const daemonChanged = daemonPid !== undefined && daemonPid !== s.pid;
          daemonPid = s.pid;
          if (daemonChanged) modulesRevRef.current = null;
          if (modulesRevRef.current !== s.modules_rev) void refreshModules();
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
  }, [client, refreshModules]);

  const keyboardConnected = v2UsbProtocol === "ofv2" || isKeyboardConnected(status);

  const pushConfigPatch = useCallback(
    async (patch: Partial<KeyglassConfig>) => {
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

  const runKeyboardPush = async (force: boolean) => {
    setSyncing(true);
    setToastMsg(null);
    try {
      const res = await v2Client.syncToKeyboard(force);
      if (!res.ok || res.persisted_verified === false) {
        setToastMsg(res.persisted_error ?? "Couldn't verify the settings on the keyboard.");
        return;
      }
      if (res.persisted_verified !== true) {
        setToastMsg(`Sent ${res.uploaded_files.length} files; not verified on the keyboard.`);
        return;
      }
      const changed = `${res.uploaded_files.length} sent, ${res.skipped_unchanged ?? 0} unchanged`;
      if (res.runtime_applied === true) {
        setToastMsg(`Settings active on keyboard (${changed}).`);
      } else {
        setToastMsg(`Settings verified on keyboard; display activation not confirmed (${changed}).`);
      }
    } catch (err) {
      setToastMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleAdoptFromKeyboard = async () => {
    setSyncConflict(null);
    setSyncing(true);
    setToastMsg(null);
    try {
      const adopted = await v2Client.adoptFromKeyboard();
      await refreshModules();
      await refreshActiveProfile();
      const extras = [
        adopted.adopted_modules && adopted.adopted_modules.length > 0
          ? `${adopted.adopted_modules.length} modules`
          : null,
        adopted.downloaded_media.length > 0 ? `${adopted.downloaded_media.length} media` : null,
      ]
        .filter(Boolean)
        .join(", ");
      setToastMsg(
        `Copied keyboard setup onto this computer (${adopted.enabled_modules} active${extras ? ` · downloaded ${extras}` : ""}).`,
      );
    } catch (err) {
      setToastMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSyncing(false);
    }
  };

  const handleSyncToKeyboard = async () => {
    setSyncing(true);
    setToastMsg(null);
    try {
      const chk = await v2Client.checkKeyboardSync().catch(() => null);
      if (chk?.conflict && chk.remote) {
        setSyncConflict({
          remoteHost: chk.remote.host_name || chk.remote.host_id,
          enabledCount: chk.remote.enabled_modules.length,
          bgMedia: chk.remote.bg_media,
        });
        setSyncing(false);
        return;
      }
    } catch {
      // fall through to normal sync
    }
    await runKeyboardPush(false);
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
      <aside className="kg-sidebar" aria-label="Keyglass Modules">
        <button
          type="button"
          className={`kg-brand-pill ${view.kind === "home" ? "is-active" : ""}`}
          onClick={() => setView({ kind: "home" })}
          title="Return to live keyboard overview"
        >
          <span className="kg-brand-icon">◇</span>
          <span className="kg-brand-name">Keyglass</span>
          <span className={`kg-conn-dot ${keyboardConnected ? "is-on" : ""}`} />
          <span className={`kg-conn-text ${keyboardConnected ? "is-on" : ""}`}>
            {keyboardConnected ? "Connected" : "Offline"}
          </span>
        </button>

        <div className="kg-sidebar-section-header">
          <span className="kg-sidebar-section-label">Modules</span>
          <button
            type="button"
            className={`kg-sidebar-manage-btn ${view.kind === "store" ? "is-active" : ""}`}
            onClick={() => setView({ kind: "store" })}
            title="Manage, install, update, or remove modules"
          >
            + Manage
          </button>
        </div>
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
                <span className="kg-nav-glyph">
                  <ModuleIcon mod={m} />
                </span>
                <span className="kg-nav-label">{sidebarLabel(m)}</span>
                {!m.healthy && <span className="kg-err-dot" title={m.last_error ?? "Module error"} />}
              </button>
            );
          })}
        </nav>

        <div className="kg-sidebar-footer" style={{ position: "relative" }}>
          {activeProfile && (() => {
            const stack = activeProfile.stack ?? [];
            const osLayer = stack.find((s) => s.scope === "os");
            const appLayer = [...stack].reverse().find((s) => s.scope === "app");
            const hostLayer = stack.find((s) => s.scope === "host");
            const osLabel =
              osLayer?.name ??
              (activeProfile.host?.os === "windows"
                ? "Windows"
                : activeProfile.host?.os === "macos"
                  ? "macOS"
                  : activeProfile.host?.os || "Desktop");
            const layerSuffix = activeProfile.qmk_layer != null ? ` (L${activeProfile.qmk_layer})` : "";
            const chipText = activeProfile.manual_override
              ? `📌 ${activeProfile.profile_name}${layerSuffix}`
              : appLayer
                ? `${appLayer.name} · ${osLabel}${layerSuffix}`
                : hostLayer
                  ? `${hostLayer.name} · ${osLabel}${layerSuffix}`
                  : `${osLabel}${layerSuffix} · Shared`;
            return (
              <div style={{ marginBottom: 6 }}>
                {profileMenuOpen && (
                  <div
                    className="kg-calm-card"
                    style={{
                      position: "absolute",
                      left: 8,
                      right: 8,
                      bottom: "calc(100% + 6px)",
                      zIndex: 40,
                      padding: 10,
                      display: "flex",
                      flexDirection: "column",
                      gap: 6,
                      boxShadow: "0 12px 32px rgba(0, 0, 0, 0.55)",
                      borderColor: "rgba(92, 225, 255, 0.28)",
                    }}
                  >
                    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                      <span className="kg-muted" style={{ fontSize: 10, fontWeight: 700, textTransform: "uppercase", letterSpacing: "0.08em" }}>
                        {activeProfile.manual_override ? "Pinned Overlay Stack" : "Live Overlay Stack"}
                      </span>
                      <button
                        type="button"
                        className="kg-btn-ghost"
                        style={{ padding: "1px 6px", fontSize: 10 }}
                        onClick={() => setProfileMenuOpen(false)}
                      >
                        ✕
                      </button>
                    </div>
                    {stack.map((layer, idx) => (
                      <button
                        key={`${layer.scope}-${layer.id}`}
                        type="button"
                        className="kg-nav-item"
                        style={{ padding: "5px 8px", textAlign: "left" }}
                        onClick={() => {
                          setProfileMenuOpen(false);
                          setView({ kind: "module", moduleId: "profiles", focusProfileId: layer.id });
                        }}
                      >
                        <span style={{ flex: 1 }}>
                          <div style={{ fontSize: 12, fontWeight: 600 }}>
                            {idx > 0 ? "↳ " : ""}
                            {layer.pinned ? "📌 " : ""}
                            {layer.name}
                          </div>
                          <div className="kg-muted" style={{ fontSize: 10 }}>
                            {layer.summary}
                          </div>
                        </span>
                      </button>
                    ))}
                    <div style={{ display: "flex", gap: 6, marginTop: 4 }}>
                      {activeProfile.manual_override && (
                        <button
                          type="button"
                          className="kg-btn-primary"
                          style={{ flex: 1, padding: "4px 8px", fontSize: 11 }}
                          onClick={async () => {
                            await v2Client.selectProfile(null);
                            await refreshActiveProfile();
                            setProfileMenuOpen(false);
                          }}
                        >
                          Return to Auto
                        </button>
                      )}
                      <button
                        type="button"
                        className="kg-btn-ghost"
                        style={{ flex: 1, padding: "4px 8px", fontSize: 11 }}
                        onClick={() => {
                          setProfileMenuOpen(false);
                          setView({ kind: "module", moduleId: "profiles" });
                        }}
                      >
                        Open Profiles →
                      </button>
                    </div>
                  </div>
                )}
                <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                  <button
                    type="button"
                    className="kg-nav-item"
                    style={{
                      flex: 1,
                      padding: "6px 10px",
                      fontSize: 11,
                      background: activeProfile.manual_override
                        ? "rgba(92, 225, 255, 0.12)"
                        : "rgba(255, 255, 255, 0.04)",
                      border: activeProfile.manual_override
                        ? "1px solid rgba(92, 225, 255, 0.35)"
                        : "1px solid rgba(255, 255, 255, 0.08)",
                    }}
                    onClick={() => setProfileMenuOpen((v) => !v)}
                    title="Inspect active profile overlay stack"
                  >
                    <span className="kg-conn-dot is-on" />
                    <span className="kg-nav-label" style={{ fontSize: 11 }}>
                      {chipText}
                    </span>
                  </button>
                  {activeProfile.manual_override && (
                    <button
                      type="button"
                      className="kg-btn-ghost"
                      style={{ padding: "5px 7px", fontSize: 10 }}
                      title="Return to automatic profile matching"
                      onClick={async () => {
                        await v2Client.selectProfile(null);
                        await refreshActiveProfile();
                      }}
                    >
                      Auto
                    </button>
                  )}
                </div>
              </div>
            );
          })()}
          <button
            type="button"
            className={`kg-nav-item ${view.kind === "store" ? "is-active" : ""}`}
            onClick={() => setView({ kind: "store" })}
          >
            <span className="kg-nav-glyph" aria-hidden="true">
              ⊞
            </span>
            <span className="kg-nav-label">Manage Modules</span>
          </button>
          <button
            type="button"
            className="kg-nav-item"
            disabled={syncing}
            onClick={() => void handleSyncToKeyboard()}
          >
            <span className="kg-nav-glyph" aria-hidden="true">
              ↻
            </span>
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
        {toastMsg && <div className="kg-toast-banner" role="status">{toastMsg}</div>}

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
            onOpenProfile={(pid) => setView({ kind: "module", moduleId: "profiles", focusProfileId: pid })}
          />
        )}

        {view.kind === "module" && view.moduleId === "keys-custom" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Custom Legends</h1>
                <span className="kg-pill-subtle">What each key shows, and when</span>
              </div>
            </div>
            <KeycapsTab pickFile={(title) => client.pickFile(title)} initialKey={view.focusKey} />
          </div>
        )}

        {view.kind === "module" && view.moduleId === "dock-top-bar" && (
          <div className="kg-module-page">
            <div className="kg-page-header">
              <div className="kg-page-title-row">
                <h1>Top Bar &amp; Attachments</h1>
                <span className="kg-pill-subtle">Module · Physical Bays &amp; Widgets</span>
              </div>
            </div>
            <TopBarDockTab keys={geomKeys} onModulesChanged={setModules} />
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

        {view.kind === "module" && view.moduleId === "profiles" && (
          <ProfilesView
            modules={modules}
            initialSelectedId={view.focusProfileId}
            onActiveChanged={() => void refreshActiveProfile()}
          />
        )}

        {view.kind === "module" && view.moduleId === "streamdeck-bridge" && (
          <StreamDeckView
            pickFile={(title, exts) => client.pickFile(title, exts)}
            pickFolder={(title) => client.pickFolder(title)}
            onOpenKeymap={(k) => openModule("keys-keymap", k)}
          />
        )}

        {view.kind === "module" &&
          activeMod &&
          !["keys-keymap", "keys-custom", "dock-top-bar", "keys-shortcuts", "profiles", "streamdeck-bridge"].includes(view.moduleId) && (
            <ModuleView
              mod={activeMod}
              keys={geomKeys}
              activeProfile={activeProfile}
              pickFile={(title) => client.pickFile(title)}
              onUpdated={refreshModules}
              onOpenKeymap={(k) => openModule("keys-keymap", k)}
              onOpenProfile={(pid) => setView({ kind: "module", moduleId: "profiles", focusProfileId: pid })}
              onUninstalled={() => setView({ kind: "home" })}
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
                <span className="kg-pill-subtle">Keyglass · Hardware &amp; Optical Calibration</span>
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

      {/* Multi-Computer Sync Conflict Modal (3 unmistakable explicit buttons — zero OK/Cancel guessing!) */}
      {syncConflict && (
        <div
          role="dialog"
          aria-modal="true"
          aria-labelledby="kg-sync-conflict-title"
          style={{
            position: "fixed",
            inset: 0,
            zIndex: 100,
            background: "rgba(4, 7, 13, 0.72)",
            backdropFilter: "blur(6px)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            padding: 20,
          }}
          onClick={() => setSyncConflict(null)}
        >
          <div
            className="kg-calm-card"
            style={{
              width: "100%",
              maxWidth: 460,
              padding: 22,
              display: "flex",
              flexDirection: "column",
              gap: 14,
              borderColor: "rgba(92, 225, 255, 0.35)",
              boxShadow: "0 24px 64px rgba(0, 0, 0, 0.65)",
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div>
              <h2 id="kg-sync-conflict-title" style={{ margin: 0, fontSize: 17 }}>
                Keyboard Setup From Another Computer
              </h2>
              <p className="kg-muted" style={{ margin: "8px 0 0", fontSize: 13, lineHeight: 1.45 }}>
                This keyboard is currently running a setup from{" "}
                <strong style={{ color: "#eef4ff" }}>{syncConflict.remoteHost}</strong> ({syncConflict.enabledCount}{" "}
                active modules
                {syncConflict.bgMedia ? ` · background: ${syncConflict.bgMedia}` : ""}).
              </p>
            </div>

            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <button
                type="button"
                className="kg-btn-primary"
                style={{ width: "100%", textAlign: "left", padding: "10px 14px" }}
                onClick={() => void handleAdoptFromKeyboard()}
              >
                <div style={{ fontWeight: 600 }}>Copy Setup from Keyboard to This Computer</div>
                <div style={{ fontSize: 11, opacity: 0.85, marginTop: 2 }}>
                  Recommended — keeps your keyboard’s video, modules, and settings.
                </div>
              </button>

              <button
                type="button"
                className="kg-btn-ghost kg-btn-danger-ghost"
                style={{ width: "100%", textAlign: "left", padding: "10px 14px" }}
                onClick={() => {
                  setSyncConflict(null);
                  void runKeyboardPush(true);
                }}
              >
                <div style={{ fontWeight: 600 }}>Overwrite Keyboard with This Computer</div>
                <div style={{ fontSize: 11, opacity: 0.8, marginTop: 2 }}>
                  Replaces what’s on the keyboard with this computer’s current setup.
                </div>
              </button>

              <button
                type="button"
                className="kg-btn-ghost"
                style={{ width: "100%", padding: "8px 14px" }}
                onClick={() => {
                  setSyncConflict(null);
                  setToastMsg("Sync canceled — keyboard setup untouched.");
                }}
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Hidden WebView host for HTML/JS `.sdPlugin` packages (e.g. Task Timer's index.html) */}
      {htmlSdPlugins.map((hp) => (
        <iframe
          key={hp.id}
          src={hp.url}
          title={`StreamDeck Plugin ${hp.id}`}
          style={{ position: "fixed", width: 1, height: 1, opacity: 0.001, pointerEvents: "none", border: "none" }}
        />
      ))}
    </div>
  );
};

export default App;
