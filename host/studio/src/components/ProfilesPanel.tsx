import React, { useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import type { AssetCatalog, InstalledAppInfo, OpenFluxConfig, ShortcutProfile } from "../types/openflux";

interface ProfilesPanelProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  config: OpenFluxConfig;
  effectiveActiveIndex?: number;
  onChangeProfiles: (profiles: ShortcutProfile[], desiredProfileIndex?: number) => void;
  onSelectProfile: (index: number) => void;
  /** `macos`, `windows`, `linux`. The daemon lists installed apps on macOS only. */
  platform?: string;
}

export const ProfilesPanel: React.FC<ProfilesPanelProps> = ({
  client,
  catalog,
  config,
  effectiveActiveIndex,
  onChangeProfiles,
  onSelectProfile,
  platform,
}) => {
  const canListApps = platform === undefined || platform === "macos";
  const profiles: ShortcutProfile[] = config.profiles?.length
    ? config.profiles
    : [
        {
          id: "default",
          name: "Default",
          theme_id: "default",
          contexts: ["Main"],
          active_context: "Main",
          actuation_mode_index: 0,
          default_layer: 0,
        },
      ];
  const activeIdx = Math.min(effectiveActiveIndex ?? config.desiredProfileIndex ?? 0, Math.max(0, profiles.length - 1));
  const [inspectIdx, setInspectIdx] = useState<number>(activeIdx);
  React.useEffect(() => {
    setInspectIdx(activeIdx);
  }, [activeIdx]);
  const safeInspectIdx = Math.min(inspectIdx, Math.max(0, profiles.length - 1));
  const selected = profiles[safeInspectIdx] ?? profiles[0];

  const [newName, setNewName] = useState("");
  const [newContextText, setNewContextText] = useState("");
  const [showAppPicker, setShowAppPicker] = useState(false);
  const [installedApps, setInstalledApps] = useState<InstalledAppInfo[]>([]);
  const [appQuery, setAppQuery] = useState("");

  const layout = config.keymap.selectedFrameLayoutType;
  const layers = config.keymap.keymaps[layout]?.layers ?? [{ name: "Base", keyActions: [] }];
  const modes = config.actuationModes ?? [];
  const packs = catalog?.shortcutPacks ?? [];

  const themes = [
    { id: "default", name: config.appearance.defaultTheme.name || "Default" },
    ...(config.appearance.customThemes ?? []).map((t) => ({ id: t.id, name: t.name })),
  ];

  const patchSelected = (patch: Partial<ShortcutProfile>) => {
    const next = profiles.map((p, i) => (i === safeInspectIdx ? { ...p, ...patch } : p));
    onChangeProfiles(next);
  };

  const createProfile = () => {
    const trimmed = newName.trim();
    if (!trimmed) return;
    const baseSlug =
      trimmed
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "_")
        .replace(/^_|_$/g, "") || "profile";
    let uid = baseSlug;
    let suffix = 2;
    while (profiles.some((p) => p.id === uid)) {
      uid = `${baseSlug}_${suffix++}`;
    }
    const created: ShortcutProfile = {
      id: uid,
      name: trimmed,
      theme_id: "default",
      contexts: ["Main"],
      active_context: "Main",
      actuation_mode_index: 0,
      default_layer: 0,
      shortcut_pack: null,
      app_matcher: null,
    };
    const next = [...profiles, created];
    setNewName("");
    setInspectIdx(next.length - 1);
    onChangeProfiles(next, next.length - 1);
  };

  const duplicateProfile = () => {
    const base = selected;
    let uid = `${base.id}_copy`;
    let suffix = 2;
    while (profiles.some((p) => p.id === uid)) {
      uid = `${base.id}_copy_${suffix++}`;
    }
    const copy: ShortcutProfile = {
      ...base,
      id: uid,
      name: `${base.name} Copy`,
      contexts: [...base.contexts],
    };
    const next = [...profiles, copy];
    setInspectIdx(next.length - 1);
    onChangeProfiles(next, next.length - 1);
  };

  const deleteSelected = () => {
    if (profiles.length <= 1) return;
    const manualIdx = Math.min(config.desiredProfileIndex ?? 0, Math.max(0, profiles.length - 1));
    const manualId = profiles[manualIdx]?.id;
    const next = profiles.filter((_, i) => i !== safeInspectIdx);
    const nextManual = next.findIndex((p) => p.id === manualId);
    setInspectIdx(Math.max(0, safeInspectIdx - 1));
    onChangeProfiles(next, Math.max(nextManual, 0));
  };

  return (
    <div className="workbench-3col">
      {/* Left Column: Named Profiles List */}
      <div className="card col-scroll">
        <div className="toolbar-between">
          <h3>Profiles ({profiles.length})</h3>
          <span className="pill">Auto-Switch Ready</span>
        </div>
        <p className="muted" style={{ fontSize: 12, marginTop: 4, marginBottom: 10 }}>
          A profile bundles a Theme, Actuation feel, Base Layer, and Shortcut overlay pack—and can auto-activate when
          you switch apps.
        </p>

        <div className="profile-create-row">
          <input
            type="text"
            placeholder="New profile name..."
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                createProfile();
              }
            }}
          />
          <button type="button" className="btn-primary" onClick={createProfile}>
            New Profile
          </button>
        </div>

        <ul className="profile-list" aria-label="Named Profiles">
          {profiles.map((p, idx) => {
            const isActive = idx === activeIdx;
            const isInspected = idx === safeInspectIdx;
            const themeName = themes.find((t) => t.id === p.theme_id)?.name ?? p.theme_id;
            const pModeIdx = p.actuation_mode_index ?? 0;
            const pLayerIdx = p.default_layer ?? 0;
            const modeName = modes[pModeIdx]?.name ?? `Mode #${pModeIdx}`;
            return (
              <li key={p.id}>
                <button
                  type="button"
                  aria-pressed={isInspected}
                  className={`profile-row ${isInspected ? "selected" : ""}`}
                  onClick={() => {
                    setInspectIdx(idx);
                    onSelectProfile(idx);
                  }}
                >
                  <span className="profile-row-head">
                    <strong>{p.name}</strong>
                    {isActive && <span className="pill">Active</span>}
                  </span>
                  <small className="muted" title={`Theme: ${themeName} · ${modeName} · L${pLayerIdx}`}>
                    Theme: {themeName} · {modeName} · L{pLayerIdx}
                  </small>
                  {p.app_matcher && <small className="profile-matcher-chip">{p.app_matcher}</small>}
                </button>
              </li>
            );
          })}
        </ul>
      </div>

      {/* Center Column: Selected Profile Overview & Contexts */}
      <div className="card col-scroll">
        <div className="toolbar-between">
          <h3>{selected.name} — Summary</h3>
          <div className="btn-group">
            <button type="button" className="btn-secondary" onClick={duplicateProfile}>
              Duplicate
            </button>
            <button type="button" className="btn-secondary" disabled={profiles.length <= 1} onClick={deleteSelected}>
              Delete
            </button>
          </div>
        </div>

        <div className="kv-list" style={{ marginBottom: 14 }}>
          <div>
            <span>Profile ID</span>
            <strong>
              <code>{selected.id}</code>
            </strong>
          </div>
          <div>
            <span>Bound Theme</span>
            <strong>{themes.find((t) => t.id === selected.theme_id)?.name ?? selected.theme_id}</strong>
          </div>
          <div>
            <span>Actuation Mode</span>
            <strong>
              {modes[selected.actuation_mode_index ?? 0]?.name ?? `Mode #${selected.actuation_mode_index ?? 0}`}
            </strong>
          </div>
          <div>
            <span>Default Keymap Layer</span>
            <strong>
              L{selected.default_layer ?? 0}: {layers[selected.default_layer ?? 0]?.name ?? "Base"}
            </strong>
          </div>
          <div>
            <span>Shortcut Pack</span>
            <strong>
              {selected.shortcut_pack
                ? (packs.find((pk) => pk.id === selected.shortcut_pack)?.name ?? selected.shortcut_pack)
                : "None"}
            </strong>
          </div>
          <div>
            <span>Auto-Switch App</span>
            <strong>{selected.app_matcher || "Manual only"}</strong>
          </div>
        </div>

        <h4>Shortcut Contexts</h4>
        <div className="btn-group" style={{ marginBottom: 10 }}>
          {selected.contexts.map((ctx) => (
            <button
              key={ctx}
              type="button"
              className={`btn-secondary ${selected.active_context === ctx ? "active" : ""}`}
              onClick={() => patchSelected({ active_context: ctx })}
            >
              {ctx}
            </button>
          ))}
        </div>
        <div className="btn-group">
          <input
            type="text"
            placeholder="Add context (e.g. Sculpt)"
            value={newContextText}
            onChange={(e) => setNewContextText(e.target.value)}
            style={{ flex: 1 }}
          />
          <button
            type="button"
            className="btn-secondary"
            onClick={() => {
              const c = newContextText.trim();
              if (!c || selected.contexts.includes(c)) return;
              patchSelected({ contexts: [...selected.contexts, c], active_context: c });
              setNewContextText("");
            }}
          >
            Add Context
          </button>
        </div>
      </div>

      {/* Right Column: Profile Bindings Inspector */}
      <div className="card col-scroll">
        <h3>Profile Bindings</h3>
        <label className="field">
          <span>Profile Name</span>
          <input
            type="text"
            aria-label="Profile Name"
            value={selected.name}
            onChange={(e) => patchSelected({ name: e.target.value })}
          />
        </label>

        <label className="field">
          <span>Bound Theme</span>
          <select
            aria-label="Bound Theme"
            value={selected.theme_id}
            onChange={(e) => patchSelected({ theme_id: e.target.value })}
          >
            {themes.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span>Bound Actuation Mode</span>
          <select
            aria-label="Bound Actuation Mode"
            value={selected.actuation_mode_index}
            onChange={(e) => patchSelected({ actuation_mode_index: Number(e.target.value) })}
          >
            {modes.map((m, i) => (
              <option key={m.id} value={i}>
                {m.name} ({m.actuationPointMm.toFixed(1)} mm)
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span>Default Layer</span>
          <select
            aria-label="Bound Default Layer"
            value={selected.default_layer}
            onChange={(e) => patchSelected({ default_layer: Number(e.target.value) })}
          >
            {layers.map((l, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: firmware layers are addressed by position, so the index is the layer's identity.
              <option key={`${l.name}-${i}`} value={i}>
                L{i}: {l.name}
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span>Shortcut CSV Pack</span>
          <select
            aria-label="Bound Shortcut Pack"
            value={selected.shortcut_pack ?? ""}
            onChange={(e) => patchSelected({ shortcut_pack: e.target.value || null })}
          >
            <option value="">None</option>
            {packs.map((pk) => (
              <option key={pk.id} value={pk.id}>
                {pk.name}
              </option>
            ))}
          </select>
        </label>

        <div className="field">
          <div className="toolbar-between" style={{ marginBottom: 4 }}>
            <span>Auto-Switch Application Matcher</span>
            {canListApps && (
              <button
                type="button"
                className="btn-secondary"
                onClick={async () => {
                  setShowAppPicker(true);
                  try {
                    const apps = await client.getInstalledApps();
                    setInstalledApps(apps);
                  } catch {
                    setInstalledApps([]);
                  }
                }}
              >
                Pick App
              </button>
            )}
          </div>
          <input
            type="text"
            aria-label="Profile App Matcher"
            placeholder="e.g. com.blenderfoundation.blender"
            value={selected.app_matcher ?? ""}
            onChange={(e) => patchSelected({ app_matcher: e.target.value || null })}
          />
        </div>
      </div>

      {showAppPicker && (
        <div
          className="modal-backdrop"
          role="dialog"
          aria-label="Select Application"
          onClick={(e) => {
            if (e.target === e.currentTarget) setShowAppPicker(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") setShowAppPicker(false);
          }}
        >
          <div className="modal-card">
            <div className="toolbar-between">
              <h3>Select Application</h3>
              <button type="button" className="btn-secondary" onClick={() => setShowAppPicker(false)}>
                Close
              </button>
            </div>
            <input
              type="text"
              placeholder="Filter apps or bundle ID..."
              value={appQuery}
              onChange={(e) => setAppQuery(e.target.value)}
              style={{ marginBottom: 10 }}
            />
            <div className="modal-list">
              {installedApps
                .filter(
                  (a) =>
                    a.name.toLowerCase().includes(appQuery.toLowerCase()) ||
                    a.bundleId.toLowerCase().includes(appQuery.toLowerCase()),
                )
                .slice(0, 80)
                .map((app) => (
                  <button
                    key={`${app.bundleId}-${app.path}`}
                    type="button"
                    className="modal-list-row"
                    onClick={() => {
                      patchSelected({ app_matcher: app.bundleId });
                      setShowAppPicker(false);
                    }}
                  >
                    <strong>{app.name}</strong>
                    <code>{app.bundleId}</code>
                  </button>
                ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
