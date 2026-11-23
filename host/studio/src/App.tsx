import type React from "react";
import { useEffect, useRef, useState } from "react";
import type { DaemonClient } from "./api/daemonClient";
import { createDaemonClient } from "./api/daemonClient";
import { ActuationPanel } from "./components/ActuationPanel";
import { DevicePanel } from "./components/DevicePanel";
import { OpenFluxLogo } from "./components/OpenFluxLogo";
import { ShortcutsProfilesPanel } from "./components/ShortcutsProfilesPanel";
import { TopBarDockTab } from "./components/TopBarDockTab";
import { V2ModulesHub } from "./components/V2ModulesHub";
import { VialKeymapTab } from "./components/VialKeymapTab";
import { type V2ModuleRecord, v2Client } from "./api/v2Client";
import type { AssetCatalog, DeviceStatus, OpenFluxConfig, PreviewKeyRect } from "./types/openflux";
import { isKeyboardConnected, PLACEHOLDER_CONFIG } from "./types/openflux";

/** Pure OpenFlux v2 core pages (Modules hub + RP2040/Vial hardware tabs). Software modules opt into dynamic top tabs! */
const CORE_NAV = [
  { id: "v2", label: "Modules" },
  { id: "actuation", label: "Actuation" },
  { id: "keymap", label: "Keymap" },
  { id: "firmware", label: "Device" },
] as const;

export type CoreStudioTab = (typeof CORE_NAV)[number]["id"];
export type StudioTab = CoreStudioTab | `mod:${string}` | "profiles";

interface AppProps {
  client?: DaemonClient;
  initialConfig?: OpenFluxConfig;
  initialTab?: StudioTab;
  /** `macos`, `windows`, `linux`: adjusts platform-specific explanations. */
  platform?: string;
}

export const App: React.FC<AppProps> = ({
  client: injectedClient,
  initialConfig,
  initialTab,
  platform,
}) => {
  const [client] = useState<DaemonClient>(() => injectedClient ?? createDaemonClient());
  const [tab, setTab] = useState<StudioTab>(initialTab ?? "v2");
  const [v2Modules, setV2Modules] = useState<V2ModuleRecord[]>([]);

  useEffect(() => {
    void v2Client
      .listModules()
      .then((res) => setV2Modules(res.modules))
      .catch(() => {
        // Ignore in offline unit tests
      });
  }, []);

  // Enabled modules that opt into a dedicated top-level Studio tab (via `manifest.ui.top_tab` or user `pin_tab`)
  const moduleTabs = v2Modules
    .filter((m) => m.state.enabled && (m.state.pin_tab ?? Boolean(m.manifest.ui?.top_tab)))
    .sort((a, b) => (a.manifest.ui?.top_tab_order ?? 50) - (b.manifest.ui?.top_tab_order ?? 50));
  const [config, setConfig] = useState<OpenFluxConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  const latestServerConfigRef = useRef<OpenFluxConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  const [status, setStatus] = useState<DeviceStatus | null>(null);
  const [catalog, setCatalog] = useState<AssetCatalog | null>(null);
  const [previewRects, setPreviewRects] = useState<PreviewKeyRect[]>([]);
  const [activeLayer, setActiveLayer] = useState(0);
  const [selectedKeyIndex, setSelectedKeyIndex] = useState<number | null>(0);
  const [dirty, setDirty] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);
  const [, setSyncedRevision] = useState(0);
  const pendingRevRef = useRef(0);
  const sectionRevRef = useRef<Record<string, number>>({});
  const failedSectionsRef = useRef<Set<string>>(new Set());
  const refreshEpochRef = useRef(0);
  const appliedEpochRef = useRef(0);
  const importCountRef = useRef(0);
  const importNeedsRefreshRef = useRef(false);
  const importBarrierEpochRef = useRef(0);
  const flushChainRef = useRef<Promise<void>>(Promise.resolve());
  const configRef = useRef<OpenFluxConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  configRef.current = config;

  const hasLoadedDaemonSnapshotRef = useRef(Boolean(initialConfig));
  const [snapshotLoaded, setSnapshotLoaded] = useState(Boolean(initialConfig));
  const [committedLayout, setCommittedLayout] = useState(
    (initialConfig ?? PLACEHOLDER_CONFIG).keymap.selectedFrameLayoutType,
  );
  const needsHydrationRef = useRef(true);

  const normalizeConfig = (cfg: OpenFluxConfig): OpenFluxConfig => ({
    ...cfg,
    appearance: {
      ...cfg.appearance,
      defaultTheme: {
        ...cfg.appearance.defaultTheme,
        stylemaps: cfg.appearance?.defaultTheme?.stylemaps ?? {},
      },
      customThemes: (cfg.appearance?.customThemes ?? []).map((t) => ({
        ...t,
        stylemaps: t.stylemaps ?? {},
      })),
    },
  });

  const applyConfigSnapshot = (rawCfg: OpenFluxConfig, snapRevs: Record<string, number>, epoch: number) => {
    if (epoch < appliedEpochRef.current) {
      return;
    }
    const cfg = normalizeConfig(rawCfg);
    appliedEpochRef.current = epoch;
    latestServerConfigRef.current = cfg;
    hasLoadedDaemonSnapshotRef.current = true;
    setSnapshotLoaded(true);
    setCommittedLayout(cfg.keymap.selectedFrameLayoutType);
    const h = cfg.hydrated;
    needsHydrationRef.current = Boolean(h && (!h.appearance || !h.settings || !h.mosaic || !h.keymap));
    setConfig((prev) => {
      const next = { ...prev };
      for (const k of Object.keys(cfg) as (keyof OpenFluxConfig)[]) {
        if ((sectionRevRef.current[k] ?? 0) === (snapRevs[k] ?? 0) && !failedSectionsRef.current.has(k)) {
          (next as Record<string, unknown>)[k] = cfg[k];
        }
      }
      return next;
    });
  };

  const refreshFromDaemon = (barrierWait = true) => {
    const snapRevs = { ...sectionRevRef.current };
    const epoch = ++refreshEpochRef.current;
    const runRefresh = async () => {
      try {
        const cfg = await client.getConfig();
        if (epoch < appliedEpochRef.current) return;
        // A GET that started before the import barrier predates the import and must not clear it.
        if (importNeedsRefreshRef.current && epoch <= importBarrierEpochRef.current) {
          return;
        }
        applyConfigSnapshot(cfg, snapRevs, epoch);
        if (importNeedsRefreshRef.current && epoch > importBarrierEpochRef.current) {
          if (cfg.appearance?.customThemes?.length) {
            setConfig((prev) => {
              const curIds = new Set(prev.appearance.customThemes.map((t) => t.id));
              const add = cfg.appearance.customThemes.filter((t) => !curIds.has(t.id));
              if (add.length === 0) return prev;
              return {
                ...prev,
                appearance: {
                  ...prev.appearance,
                  customThemes: [...prev.appearance.customThemes, ...add],
                },
              };
            });
          }
          importCountRef.current += 1;
          importNeedsRefreshRef.current = false;
          failedSectionsRef.current.delete("import_refresh");
        }
        setSyncedRevision((r) => r + 1);
      } catch (err) {
        if (importNeedsRefreshRef.current) {
          setBanner(`Import refresh failed: ${String(err)}`);
        }
      }
      try {
        const st = await client.getStatus();
        setStatus(st);
        prevPollProfileRef.current = {
          desired: st.desiredProfileIndex,
          applied: st.appliedProfileIndex,
        };
      } catch {
        /* Status stays at its last value while the daemon is unreachable. */
      }
    };
    if (!barrierWait) {
      return runRefresh();
    }
    flushChainRef.current = flushChainRef.current.catch(() => {}).then(runRefresh);
    return flushChainRef.current;
  };

  const importBundleSerialized = (filePath: string) => {
    const snapRevs = { ...sectionRevRef.current };
    const epoch = ++refreshEpochRef.current;
    let importResult: { message: string; profiles: string[] };
    const next = flushChainRef.current
      .catch(() => {})
      .then(async () => {
        const prevCustomIds = new Set(latestServerConfigRef.current.appearance?.customThemes?.map((t) => t.id) ?? []);
        importResult = await client.importBundle(filePath);
        let cfg: OpenFluxConfig;
        try {
          cfg = await client.getConfig();
        } catch (err) {
          importNeedsRefreshRef.current = true;
          importBarrierEpochRef.current = epoch;
          failedSectionsRef.current.add("import_refresh");
          setBanner(`Imported bundle, but failed to refresh config: ${String(err)}`);
          throw err;
        }
        applyConfigSnapshot(cfg, snapRevs, epoch);
        importCountRef.current += 1;
        importNeedsRefreshRef.current = false;
        failedSectionsRef.current.delete("import_refresh");
        // Carry newly imported custom themes into pending UI state unless a reset was queued after this import.
        const newlyImported = (cfg.appearance?.customThemes ?? []).filter((t) => !prevCustomIds.has(t.id));
        if (newlyImported.length > 0 && (sectionRevRef.current.explicitReset ?? 0) === (snapRevs.explicitReset ?? 0)) {
          setConfig((prev) => {
            const curIds = new Set(prev.appearance.customThemes.map((t) => t.id));
            const add = newlyImported.filter((t) => !curIds.has(t.id));
            if (add.length === 0) return prev;
            return {
              ...prev,
              appearance: {
                ...prev.appearance,
                customThemes: [...prev.appearance.customThemes, ...add],
              },
            };
          });
        }
        try {
          const st = await client.getStatus();
          setStatus(st);
        } catch {
          /* The config snapshot is still valid when only the status call fails. */
        }
        setSyncedRevision((r) => r + 1);
        return importResult;
      });
    flushChainRef.current = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };

  const refreshCatalog = () => {
    void client
      .getAssetsCatalog()
      .then((c) => setCatalog(c))
      .catch(() => {});
  };

  const prevPollProfileRef = useRef<{ desired?: number; applied?: number | null; connected?: boolean }>({});
  const pollBackoffRef = useRef(2000);

  // biome-ignore lint/correctness/useExhaustiveDependencies: the daemon poll loop starts once on mount; the helpers it calls read only refs, state setters and the per-instance client.
  useEffect(() => {
    void refreshFromDaemon(false);
    refreshCatalog();
    let cancelled = false;
    let timerId: number | undefined;

    const scheduleNext = (ms: number) => {
      if (cancelled) return;
      timerId = window.setTimeout(() => {
        void pollTick();
      }, ms);
    };

    const pollTick = async () => {
      try {
        const st = await client.getStatus();
        if (cancelled) return;
        setStatus(st);
        const prev = prevPollProfileRef.current;
        const isConn = isKeyboardConnected(st);
        const connBecameTrue = prev.connected === false && isConn;
        const desiredChanged = prev.desired !== undefined && st.desiredProfileIndex !== prev.desired;
        const appliedChanged =
          prev.applied !== undefined && st.appliedProfileIndex !== undefined && st.appliedProfileIndex !== prev.applied;
        prevPollProfileRef.current = {
          desired: st.desiredProfileIndex,
          applied: st.appliedProfileIndex,
          connected: isConn,
        };
        if (
          desiredChanged ||
          appliedChanged ||
          connBecameTrue ||
          !hasLoadedDaemonSnapshotRef.current ||
          (isConn && needsHydrationRef.current)
        ) {
          await refreshFromDaemon();
          pollBackoffRef.current = 2000;
        } else {
          // Poll less often while nothing changes and no keyboard is attached.
          pollBackoffRef.current = isConn ? 2000 : Math.min(15000, pollBackoffRef.current * 2);
        }
      } catch {
        pollBackoffRef.current = Math.min(15000, pollBackoffRef.current * 2);
      }
      scheduleNext(pollBackoffRef.current);
    };

    scheduleNext(2000);
    return () => {
      cancelled = true;
      if (timerId !== undefined) window.clearTimeout(timerId);
      void client.closePreview().catch(() => {});
    };
  }, []);

  const [v2UsbProtocol, setV2UsbProtocol] = useState<string>("legacy");
  const keyboardConnected = v2UsbProtocol === "ofv2" || isKeyboardConnected(status);

  useEffect(() => {
    let alive = true;
    const pollV2 = () =>
      void v2Client
        .getStatus()
        .then((s) => {
          if (alive && s.usb_protocol) setV2UsbProtocol(s.usb_protocol);
        })
        .catch(() => {});
    pollV2();
    const t = window.setInterval(pollV2, 3000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);

  const lastLayoutRef = useRef(config.keymap.selectedFrameLayoutType);

  // biome-ignore lint/correctness/useExhaustiveDependencies: committedLayout refetches the rects after the daemon accepts a layout change, since an earlier request can still return the old layout.
  useEffect(() => {
    let cancelled = false;
    const layout = config.keymap.selectedFrameLayoutType;
    if (lastLayoutRef.current !== layout) {
      lastLayoutRef.current = layout;
      setPreviewRects([]);
    }
    client
      .getPreviewRects(layout)
      .then((kr) => {
        if (!cancelled) setPreviewRects(kr);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [client, config.keymap.selectedFrameLayoutType, committedLayout]);

  const isKeymapLocked = () => false;
  const keymapLocked = false;

  const pushConfigPatch = (patch: Partial<OpenFluxConfig>) => {
    const isReset = Boolean(patch.explicitReset);
    if (!isReset && !hasLoadedDaemonSnapshotRef.current) {
      setBanner("Studio is still loading the keyboard's settings. Make the change again in a moment.");
      return Promise.resolve();
    }
    const h = latestServerConfigRef.current.hydrated;
    const isConn = isKeyboardConnected(status);
    if (!isReset && isConn && status?.usbUnreadable) {
      setBanner(
        "OpenFlux sees your keyboard on USB, but couldn't communicate with it. Close any other Flux software, reconnect the keyboard, or update its firmware/bridge if it predates Windows WinUSB support.",
      );
      return Promise.resolve();
    }
    if (!isReset && isConn && h) {
      if (
        (patch.appearance && !h.appearance) ||
        (patch.settings && !h.settings) ||
        (patch.mosaic && !h.mosaic) ||
        ((patch.keymap || patch.actuationModes) && !h.keymap)
      ) {
        setBanner("Studio is still loading the keyboard's settings. Make the change again in a moment.");
        return Promise.resolve();
      }
    }
    if (!isReset && isKeymapLocked() && (patch.keymap || patch.actuationModes)) {
      setBanner("The keyboard's keymap is still loading. Make the change again when it finishes.");
      return Promise.resolve();
    }
    setConfig((prev) => ({ ...prev, ...patch }));
    setDirty(true);
    ++pendingRevRef.current;
    const enqueueImportCount = importCountRef.current;
    const keys = Object.keys(patch);
    for (const k of keys) {
      sectionRevRef.current[k] = (sectionRevRef.current[k] ?? 0) + 1;
    }
    if (isReset) {
      sectionRevRef.current.explicitReset = (sectionRevRef.current.explicitReset ?? 0) + 1;
    }
    flushChainRef.current = flushChainRef.current
      .catch(() => {})
      .then(async () => {
        if (!isReset && importNeedsRefreshRef.current && patch.appearance) {
          for (const k of keys) failedSectionsRef.current.add(k);
          setBanner(
            "This theme change wasn't applied because Studio couldn't reload the settings after the import. Reload and try again.",
          );
          return;
        }
        const actualPatch = { ...patch };
        // An import that finished after this edit was queued added themes the edit hasn't seen; keep them.
        if (
          !isReset &&
          importCountRef.current > enqueueImportCount &&
          actualPatch.appearance &&
          latestServerConfigRef.current.appearance?.customThemes?.length
        ) {
          const existingIds = new Set(actualPatch.appearance.customThemes.map((t) => t.id));
          const merged = [
            ...actualPatch.appearance.customThemes,
            ...latestServerConfigRef.current.appearance.customThemes.filter((t) => !existingIds.has(t.id)),
          ];
          actualPatch.appearance = {
            ...actualPatch.appearance,
            customThemes: merged,
          };
        }
        try {
          await client.updateConfig(actualPatch);
          latestServerConfigRef.current = { ...latestServerConfigRef.current, ...actualPatch };
          if (actualPatch.keymap?.selectedFrameLayoutType) {
            setCommittedLayout(actualPatch.keymap.selectedFrameLayoutType);
          }
          for (const k of keys) failedSectionsRef.current.delete(k);
          setSyncedRevision((r) => r + 1);
        } catch (err) {
          for (const k of keys) failedSectionsRef.current.add(k);
          setBanner(String(err));
        }
      });
    return flushChainRef.current;
  };

  const applyActiveTheme = (theme: OpenFluxConfig["appearance"]["defaultTheme"]) => {
    const curApp = configRef.current.appearance;
    const sid = curApp.selectedId;
    const isCustom = sid && sid !== "default" && curApp.customThemes.some((t) => t.id === sid);
    if (isCustom) {
      void pushConfigPatch({
        appearance: {
          ...curApp,
          customThemes: curApp.customThemes.map((t) => (t.id === sid ? theme : t)),
        },
      });
    } else {
      void pushConfigPatch({
        appearance: { ...curApp, defaultTheme: theme },
      });
    }
  };

  const handleSaveToKeyboard = () => {
    void v2Client
      .syncToKeyboard()
      .then((res) => {
        setDirty(false);
        setBanner(`Synced to keyboard over OFV2 (${res.uploaded_files.length} files, ${res.uploaded_bytes} B)`);
      })
      .catch((err) => {
        setBanner(err instanceof Error ? err.message : String(err));
      });
  };

  const exportBundleSerialized = (outPath: string): Promise<{ message: string; outPath: string }> => {
    const next = flushChainRef.current
      .catch(() => {})
      .then(async () => {
        if (failedSectionsRef.current.size > 0 || importNeedsRefreshRef.current) {
          const msg = "Cannot export: earlier config update or import refresh failed";
          setBanner(msg);
          throw new Error(msg);
        }
        return client.exportBundle(outPath);
      });
    flushChainRef.current = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };

  const restoreDefaults = async () => {
    let defaults: OpenFluxConfig;
    try {
      defaults = await client.getDefaultConfig();
    } catch (err) {
      setBanner(`Couldn't load defaults from the daemon: ${String(err)}`);
      return;
    }
    const { hydrated: _hydrated, ...sections } = defaults;
    await pushConfigPatch({ ...sections, explicitReset: true });
  };

  const activeProfiles = config.profiles ?? [];
  const manualProfileIndex = Math.min(config.desiredProfileIndex ?? 0, Math.max(0, activeProfiles.length - 1));
  const effectiveProfileIndex = Math.min(
    status?.appliedProfileIndex ?? manualProfileIndex,
    Math.max(0, activeProfiles.length - 1),
  );

  const selectProfileIndex = (idx: number) => {
    const p = activeProfiles[idx];
    if (!p) return;
    const validTheme =
      p.theme_id === "default" || (config.appearance.customThemes ?? []).some((t) => t.id === p.theme_id);
    const nextAppearance =
      validTheme && config.appearance.selectedId !== p.theme_id
        ? { ...config.appearance, selectedId: p.theme_id }
        : config.appearance;
    void pushConfigPatch({
      desiredProfileIndex: idx,
      ...(nextAppearance !== config.appearance ? { appearance: nextAppearance } : {}),
    });
    if ((p.default_layer ?? 0) >= 0) {
      setActiveLayer(p.default_layer ?? 0);
    }
  };

  return (
    <div className="studio-shell">
      <header className="topbar">
        <div className="topbar-left">
          <div className="brand">
            <span className="brand-mark">
              <OpenFluxLogo size={24} />
            </span>
            <div>
              <strong>OpenFlux Studio</strong>
            </div>
          </div>
          <select
            className="topbar-profile-select"
            aria-label="Active Profile"
            value={effectiveProfileIndex}
            onChange={(e) => selectProfileIndex(Number(e.target.value))}
          >
            {activeProfiles.map((p, idx) => (
              <option key={p.id} value={idx}>
                {idx === effectiveProfileIndex && effectiveProfileIndex !== manualProfileIndex
                  ? `Auto: ${p.name}`
                  : `Profile: ${p.name}`}
              </option>
            ))}
          </select>
        </div>

        <nav className="topbar-nav" aria-label="Studio Sections">
          <button
            type="button"
            className={`nav-btn ${tab === "v2" ? "active" : ""}`}
            onClick={() => setTab("v2")}
          >
            Modules
          </button>
          {moduleTabs.map((mod) => {
            const tabId: StudioTab = `mod:${mod.id}`;
            const label = mod.manifest.ui?.top_tab_label || mod.manifest.name;
            return (
              <button
                key={tabId}
                type="button"
                className={`nav-btn ${tab === tabId ? "active" : ""}`}
                onClick={() => setTab(tabId)}
              >
                {label}
              </button>
            );
          })}
          {CORE_NAV.filter((n) => n.id !== "v2").map(({ id, label }) => (
            <button
              key={id}
              type="button"
              className={`nav-btn ${tab === id ? "active" : ""}`}
              onClick={() => setTab(id)}
            >
              {label}
            </button>
          ))}
        </nav>

        <div className="topbar-right">
          <div className="conn-pill">
            {v2UsbProtocol === "ofv2"
              ? "Connected · OFV2"
              : keyboardConnected
                ? "Connected"
                : "Offline"}
          </div>
          <button type="button" className={`btn-primary ${dirty ? "" : "is-clean"}`} onClick={handleSaveToKeyboard}>
            Sync to Keyboard
          </button>
        </div>
      </header>
      {!snapshotLoaded && <div className="topbar-banner">Loading configuration from OpenFlux daemon...</div>}
      {status?.usbUnreadable && (
        <div className="topbar-banner">
          OpenFlux sees your keyboard on USB, but couldn't communicate with it. Close any other Flux software, reconnect
          the keyboard, or update its firmware/bridge if it predates Windows WinUSB support.
        </div>
      )}
      {banner && <div className="topbar-banner">{banner}</div>}

      <main className="main-pane">
        {tab === "v2" && (
          <V2ModulesHub
            pickFile={(title) => client.pickFile(title)}
            onModulesChanged={setV2Modules}
          />
        )}
        {tab.startsWith("mod:") &&
          (() => {
            const modId = tab.slice(4);
            const mod = v2Modules.find((m) => m.id === modId);
            if (mod?.manifest.ui?.builtin_view === "top_bar") {
              return <TopBarDockTab onModulesChanged={setV2Modules} />;
            }
            if (mod?.manifest.ui?.builtin_view === "shortcuts") {
              return (
                <ShortcutsProfilesPanel
                  client={client}
                  catalog={catalog}
                  platform={platform}
                  config={config}
                  activeLayer={activeLayer}
                  selectedKeyIndex={selectedKeyIndex}
                  keymapBlocked={keymapLocked}
                  rules={config.appRules}
                  onChangeRules={(appRules) => void pushConfigPatch({ appRules })}
                  onChangeKeymap={(keymap) => void pushConfigPatch({ keymap })}
                  onChangeAppearance={applyActiveTheme}
                  onImportBundle={importBundleSerialized}
                  onExportBundle={exportBundleSerialized}
                />
              );
            }
            return (
              <V2ModulesHub
                focusedModuleId={modId}
                pickFile={(title) => client.pickFile(title)}
                onModulesChanged={setV2Modules}
              />
            );
          })()}
        {tab === "keymap" && <VialKeymapTab />}
        {tab === "actuation" && (
          <ActuationPanel
            config={config}
            previewRects={previewRects}
            selectedKeyIndex={selectedKeyIndex}
            keymapBlocked={keymapLocked}
            onSelectKey={setSelectedKeyIndex}
            onChangeActuation={(actuationModes) => void pushConfigPatch({ actuationModes })}
            onApplyConfigEdit={(patch) => void pushConfigPatch(patch)}
          />
        )}
        {tab === "profiles" && (
          <ShortcutsProfilesPanel
            client={client}
            catalog={catalog}
            platform={platform}
            config={config}
            activeLayer={activeLayer}
            selectedKeyIndex={selectedKeyIndex}
            keymapBlocked={keymapLocked}
            rules={config.appRules}
            onChangeRules={(appRules) => void pushConfigPatch({ appRules })}
            onChangeKeymap={(keymap) => void pushConfigPatch({ keymap })}
            onChangeAppearance={applyActiveTheme}
            onImportBundle={importBundleSerialized}
            onExportBundle={exportBundleSerialized}
          />
        )}
        {tab === "firmware" && (
          <DevicePanel
            client={client}
            platform={platform}
            status={status}
            settings={config.settings}
            onChangeSettings={(settings) => void pushConfigPatch({ settings })}
            onRestoreDefaults={() => void restoreDefaults()}
          />
        )}
      </main>
    </div>
  );
};

export default App;
