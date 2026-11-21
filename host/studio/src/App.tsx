import type React from "react";
import { useEffect, useRef, useState } from "react";
import type { DaemonClient } from "./api/daemonClient";
import { createDaemonClient } from "./api/daemonClient";
import { HostUpdateFlow } from "./api/hostUpdateFlow";
import { createStudioServices, type StudioServices } from "./api/services";
import { COMPONENT_LABELS, type UpdatesStatus, updatesNeedAttention } from "./api/updates";
import { ActuationPanel } from "./components/ActuationPanel";
import { AutoProfileLockButton } from "./components/AutoProfileLockButton";
import { DevicePanel } from "./components/DevicePanel";
import { KeymapStylemapEditor } from "./components/KeymapStylemapEditor";
import { KeymapSyncBanner } from "./components/KeymapSyncBanner";
import { LivePreviewCanvas } from "./components/LivePreviewCanvas";
import { MosaicBuilder } from "./components/MosaicBuilder";
import { OpenFluxLogo } from "./components/OpenFluxLogo";
import { ProfilesPanel } from "./components/ProfilesPanel";
import { ShortcutsProfilesPanel } from "./components/ShortcutsProfilesPanel";
import { ThemeBackgroundPanel } from "./components/ThemeBackgroundPanel";
import { ThirdPartyAppsPage } from "./components/ThirdPartyAppsPage";
import { UpdatesPage } from "./components/updates/UpdatesPage";
import type { AssetCatalog, DeviceStatus, OpenFluxConfig, PreviewKeyRect } from "./types/openflux";
import { isKeyboardConnected, keymapSyncNotice, PLACEHOLDER_CONFIG } from "./types/openflux";

/** The Studio pages, in navigation order, with their navigation labels. */
const NAV = [
  { id: "named_profiles", label: "Profiles" },
  { id: "themes", label: "Themes" },
  { id: "actuation", label: "Actuation" },
  { id: "keymap", label: "Keymap" },
  { id: "profiles", label: "Shortcuts" },
  { id: "mosaic", label: "Modules" },
  { id: "firmware", label: "Device" },
  { id: "updates", label: "Updates" },
  { id: "apps", label: "Apps" },
] as const;

export type StudioTab = (typeof NAV)[number]["id"];

const isStudioTab = (page: string): page is StudioTab => NAV.some((n) => n.id === page);

interface AppProps {
  client?: DaemonClient;
  services?: StudioServices;
  initialConfig?: OpenFluxConfig;
  initialTab?: StudioTab;
  /** `macos`, `windows`, `linux`: adjusts platform-specific explanations. */
  platform?: string;
}

/** Badge polling: updates change rarely, app registrations wait for an answer. */
const UPDATES_BADGE_POLL_MS = 5 * 60_000;
const APPS_BADGE_POLL_MS = 10_000;

export const App: React.FC<AppProps> = ({
  client: injectedClient,
  services: injectedServices,
  initialConfig,
  initialTab,
  platform,
}) => {
  const [client] = useState<DaemonClient>(() => injectedClient ?? createDaemonClient());
  const [services] = useState<StudioServices>(() => injectedServices ?? createStudioServices());
  const [hostFlow] = useState(
    () =>
      new HostUpdateFlow({
        updater: services.hostUpdater,
        lifecycle: (action) => services.updates.lifecycle(action),
        currentJob: async () => (await services.updates.getUpdates()).job,
        startDaemon: (mode) => services.daemonService.start(mode),
      }),
  );
  const [tab, setTab] = useState<StudioTab>(initialTab ?? "named_profiles");
  const [updatesStatus, setUpdatesStatus] = useState<UpdatesStatus | null>(null);
  const [updatesBadge, setUpdatesBadge] = useState(false);
  const [pendingApps, setPendingApps] = useState(0);
  const [config, setConfig] = useState<OpenFluxConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  const latestServerConfigRef = useRef<OpenFluxConfig>(initialConfig ?? PLACEHOLDER_CONFIG);
  const [status, setStatus] = useState<DeviceStatus | null>(null);
  const [catalog, setCatalog] = useState<AssetCatalog | null>(null);
  const [previewRects, setPreviewRects] = useState<PreviewKeyRect[]>([]);
  const [activeLayer, setActiveLayer] = useState(0);
  const [selectedKeyIndex, setSelectedKeyIndex] = useState<number | null>(0);
  const [dirty, setDirty] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);
  const [syncedRevision, setSyncedRevision] = useState(0);
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

  // Pages requested by the launcher (the tray opens Studio at Updates or Apps).
  useEffect(() => {
    const open = (page: string) => {
      if (isStudioTab(page)) setTab(page);
    };
    void services.navigation.initialPage().then((p) => p && open(p));
    return services.navigation.subscribe(open);
  }, [services]);

  const keyboardConnected = isKeyboardConnected(status);
  const needsConversion = updatesStatus?.keyboard.mode === "needsConversion";

  // biome-ignore lint/correctness/useExhaustiveDependencies: keyboardConnected triggers an immediate update/conversion check when a keyboard is plugged in.
  useEffect(() => {
    let cancelled = false;
    const pollUpdates = () =>
      void services.updates
        .getUpdates()
        .then((s) => {
          if (cancelled) return;
          setUpdatesStatus(s);
          setUpdatesBadge(updatesNeedAttention(s));
        })
        .catch(() => {});
    const pollApps = () =>
      void services.thirdParty
        .getRegistrations()
        .then((r) => !cancelled && setPendingApps(r.pending.length))
        .catch(() => {});
    pollUpdates();
    pollApps();
    const u = window.setInterval(pollUpdates, needsConversion ? 10_000 : UPDATES_BADGE_POLL_MS);
    const a = window.setInterval(pollApps, APPS_BADGE_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(u);
      window.clearInterval(a);
    };
  }, [services, keyboardConnected, needsConversion]);

  const lastLayoutRef = useRef(config.keymap.selectedFrameLayoutType);

  // biome-ignore lint/correctness/useExhaustiveDependencies: committedLayout refetches the rects after the daemon accepts a layout change, since an earlier request can still return the old layout.
  useEffect(() => {
    let cancelled = false;
    const layout = config.keymap.selectedFrameLayoutType;
    if (lastLayoutRef.current !== layout) {
      lastLayoutRef.current = layout;
      setPreviewRects([]);
    }
    // The rects are computed in the desktop shell; the only failure is a closing window.
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

  const isKeymapLocked = () => keymapSyncNotice(status, latestServerConfigRef.current.hydrated)?.locksEditing ?? false;

  const keymapNotice = keymapSyncNotice(status, config.hydrated);
  const keymapLocked = keymapNotice?.locksEditing ?? false;

  /** Replaces the keymap file with the keyboard's firmware keymap, then reloads config and status. */
  const adoptFirmwareKeymap = async () => {
    await client.adoptFirmwareKeymap();
    await refreshFromDaemon();
  };

  const keymapBanner = (syncingText: string) =>
    keymapNotice && (
      <KeymapSyncBanner notice={keymapNotice} syncingText={syncingText} onAdoptFirmware={adoptFirmwareKeymap} />
    );

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

  const patchActiveThemePartial = (
    partial: Partial<OpenFluxConfig["appearance"]["defaultTheme"]>,
    expectedResetEpoch?: number,
    targetThemeId?: string,
  ) => {
    const curResetEpoch = sectionRevRef.current.explicitReset ?? 0;
    if (expectedResetEpoch !== undefined && expectedResetEpoch !== curResetEpoch) {
      return;
    }
    const cur = configRef.current;
    const sid = targetThemeId ?? cur.appearance.selectedId;
    if (sid === "default" || cur.appearance.defaultTheme.id === sid) {
      void pushConfigPatch({
        appearance: {
          ...cur.appearance,
          defaultTheme: { ...cur.appearance.defaultTheme, ...partial },
        },
      });
      return;
    }
    const hasCustom = cur.appearance.customThemes.some((t) => t.id === sid);
    if (!hasCustom) {
      // The target theme was deleted or reset while the caller's async work was running.
      return;
    }
    void pushConfigPatch({
      appearance: {
        ...cur.appearance,
        customThemes: cur.appearance.customThemes.map((t) => (t.id === sid ? { ...t, ...partial } : t)),
      },
    });
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
    const saveTargetRev = pendingRevRef.current;
    flushChainRef.current = flushChainRef.current
      .catch(() => {})
      .then(async () => {
        if (failedSectionsRef.current.size > 0) {
          setBanner("Cannot save: earlier config update failed");
          return;
        }
        try {
          const res = await client.saveToKeyboard();
          setBanner(res.message);
          if (pendingRevRef.current === saveTargetRev && failedSectionsRef.current.size === 0) {
            setDirty(false);
          }
        } catch (err) {
          setBanner(String(err));
        }
      });
    return flushChainRef.current;
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

  const remapAppRulesForProfiles = (
    oldProfs: NonNullable<OpenFluxConfig["profiles"]>,
    newProfs: NonNullable<OpenFluxConfig["profiles"]>,
    oldRules: OpenFluxConfig["appRules"],
  ): OpenFluxConfig["appRules"] => {
    const out: OpenFluxConfig["appRules"] = [];
    for (const r of oldRules) {
      const oldP = oldProfs[r.profile_index];
      if (!oldP) continue;
      const newIdx = newProfs.findIndex((np) => np.id === oldP.id);
      if (newIdx < 0) continue;
      const newP = newProfs[newIdx];
      const oldM = (oldP.app_matcher ?? "").trim();
      const newM = (newP.app_matcher ?? "").trim();
      const wasGenerated = Boolean(oldM) && r.matcher === oldM && r.language_code == null;
      if (wasGenerated && newM !== oldM) continue;
      out.push({ ...r, profile_index: newIdx });
    }
    newProfs.forEach((p, idx) => {
      const m = (p.app_matcher ?? "").trim();
      if (m && !out.some((r) => r.profile_index === idx && r.matcher === m)) {
        out.push({ matcher: m, profile_index: idx, language_code: null });
      }
    });
    return out;
  };

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
          <AutoProfileLockButton
            client={client}
            state={status?.autoProfileSwitch}
            onChange={(next) => setStatus((prev) => (prev ? { ...prev, autoProfileSwitch: next } : prev))}
            onError={setBanner}
          />
        </div>

        <nav className="topbar-nav" aria-label="Studio Sections">
          {NAV.map(({ id, label }) => {
            const badge =
              id === "updates" && updatesBadge
                ? "Updates available"
                : id === "apps" && pendingApps > 0
                  ? `${pendingApps} waiting for approval`
                  : null;
            return (
              <button
                key={id}
                type="button"
                className={`nav-btn ${tab === id ? "active" : ""}`}
                onClick={() => setTab(id)}
                aria-label={badge ? `${label} (${badge})` : undefined}
              >
                {label}
                {badge && (
                  <span className="nav-badge" aria-hidden="true">
                    {id === "apps" ? pendingApps : ""}
                  </span>
                )}
              </button>
            );
          })}
        </nav>

        <div className="topbar-right">
          <div className={`conn-pill ${needsConversion ? "needs-setup" : ""}`}>
            {needsConversion ? "Setup Required" : keyboardConnected ? "Connected" : "Offline"}
          </div>
          <button type="button" className={`btn-primary ${dirty ? "" : "is-clean"}`} onClick={handleSaveToKeyboard}>
            {dirty ? "Save Changes to Keyboard" : "Save to Keyboard"}
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
        {needsConversion && tab !== "updates" && (
          <section className="conversion-callout" aria-label="Convert keyboard to OpenFlux">
            <div className="conversion-callout-copy">
              <strong>Factory Flux keyboard detected — Convert to OpenFlux</strong>
              <p className="muted">
                {updatesStatus?.keyboard.components
                  .filter((c) => c.installed)
                  .map((c) => `${COMPONENT_LABELS[c.component]} ${c.installed}`)
                  .join(" · ") || "This keyboard is running its original factory software."}
                {" — "}Converting installs OpenFlux across the keyboard, keeps your factory calibration and settings,
                and can be reset to factory software anytime.
              </p>
            </div>
            <button type="button" className="btn-primary" onClick={() => setTab("updates")}>
              Convert to OpenFlux…
            </button>
          </section>
        )}
        {tab === "named_profiles" && (
          <ProfilesPanel
            client={client}
            catalog={catalog}
            platform={platform}
            config={config}
            previewNode={
              <LivePreviewCanvas
                client={client}
                config={config}
                syncedRevision={syncedRevision}
                activeLayer={activeLayer}
                selectedKeyIndex={selectedKeyIndex}
                rects={previewRects}
                onSelectKey={(k) => setSelectedKeyIndex(k)}
              />
            }
            effectiveActiveIndex={effectiveProfileIndex}
            onChangeProfiles={(profiles, nextDesired) => {
              const curProfiles = configRef.current.profiles ?? activeProfiles;
              const nextRules = remapAppRulesForProfiles(curProfiles, profiles, configRef.current.appRules);
              const idx = nextDesired ?? manualProfileIndex;
              const selP = profiles[idx];
              const curApp = configRef.current.appearance;
              const nextAppearance =
                selP && (selP.theme_id === "default" || (curApp.customThemes ?? []).some((t) => t.id === selP.theme_id))
                  ? { ...curApp, selectedId: selP.theme_id }
                  : curApp;
              void pushConfigPatch({
                profiles,
                appRules: nextRules,
                ...(nextDesired !== undefined ? { desiredProfileIndex: nextDesired } : {}),
                ...(nextAppearance !== curApp ? { appearance: nextAppearance } : {}),
              });
            }}
            onSelectProfile={selectProfileIndex}
          />
        )}
        {tab === "themes" && (
          <ThemeBackgroundPanel
            client={client}
            catalog={catalog}
            onRefreshCatalog={refreshCatalog}
            config={config}
            resetEpoch={sectionRevRef.current.explicitReset ?? 0}
            previewNode={
              <LivePreviewCanvas
                client={client}
                config={config}
                syncedRevision={syncedRevision}
                activeLayer={activeLayer}
                selectedKeyIndex={selectedKeyIndex}
                rects={previewRects}
                onSelectKey={(k) => setSelectedKeyIndex(k)}
              />
            }
            onChangeAppearance={applyActiveTheme}
            onSelectTheme={(id) =>
              void pushConfigPatch({
                appearance: { ...configRef.current.appearance, selectedId: id },
                themeSelection: true,
              })
            }
            onCreateCustomTheme={(theme) =>
              void pushConfigPatch({
                appearance: {
                  ...configRef.current.appearance,
                  selectedId: theme.id,
                  customThemes: [...(configRef.current.appearance.customThemes ?? []), theme],
                },
              })
            }
            onPatchDefaultTheme={patchActiveThemePartial}
          />
        )}
        {tab === "keymap" && keymapBanner("Loading the keymap from the keyboard. You can edit it once it has loaded.")}
        {tab === "keymap" && (
          <KeymapStylemapEditor
            client={client}
            catalog={catalog}
            previewRects={previewRects}
            config={config}
            activeLayer={activeLayer}
            selectedKeyIndex={selectedKeyIndex}
            keymapBlocked={keymapLocked}
            onSelectLayer={setActiveLayer}
            onSelectKey={setSelectedKeyIndex}
            onChangeAppearance={applyActiveTheme}
            onChangeKeymap={(keymap) => void pushConfigPatch({ keymap })}
            onApplyConfigEdit={(patch) => void pushConfigPatch(patch)}
          />
        )}
        {tab === "actuation" && keymapBanner("Loading actuation modes and key legends from the keyboard…")}
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
        {tab === "mosaic" && (
          <MosaicBuilder
            client={client}
            mosaic={config.mosaic}
            onChangeMosaic={(mosaic) => void pushConfigPatch({ mosaic })}
            previewNode={
              <LivePreviewCanvas
                client={client}
                config={config}
                syncedRevision={syncedRevision}
                activeLayer={activeLayer}
                selectedKeyIndex={selectedKeyIndex}
                rects={previewRects}
                onSelectKey={(k) => setSelectedKeyIndex(k)}
              />
            }
          />
        )}
        {tab === "profiles" && keymapLocked && keymapBanner("Syncing shortcuts from attached keyboard…")}
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
        {tab === "updates" && (
          <UpdatesPage
            updates={services.updates}
            hostFlow={hostFlow}
            service={services.daemonService}
            onStatus={(s) => {
              setUpdatesStatus(s);
              setUpdatesBadge(updatesNeedAttention(s));
            }}
            onKeyboardChanged={() => {
              void refreshFromDaemon();
              refreshCatalog();
            }}
          />
        )}
        {tab === "apps" && (
          <ThirdPartyAppsPage client={services.thirdParty} onPendingCount={setPendingApps} platform={platform} />
        )}
      </main>
    </div>
  );
};

export default App;
