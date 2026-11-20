import type React from "react";
import { useEffect, useMemo, useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import type {
  AppearanceTheme,
  AppProfileRule,
  AssetCatalog,
  InstalledAppInfo,
  KeymapConfig,
  OpenFluxConfig,
  ShortcutPackRow,
} from "../types/openflux";
import { humanKeyName } from "../types/openflux";

interface ShortcutsProfilesPanelProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  config: OpenFluxConfig;
  activeLayer: number;
  selectedKeyIndex: number | null;
  keymapBlocked?: boolean;
  rules: AppProfileRule[];
  onChangeRules: (next: AppProfileRule[]) => void;
  onChangeKeymap: (keymap: KeymapConfig) => void;
  onChangeAppearance: (theme: AppearanceTheme) => void;
  onImportBundle: (filePath: string) => Promise<{ message: string; profiles: string[] }>;
  onExportBundle: (outPath: string) => Promise<{ message: string; outPath: string }>;
  /** `macos`, `windows`, `linux`. The daemon lists installed apps on macOS only. */
  platform?: string;
}

export const ShortcutsProfilesPanel: React.FC<ShortcutsProfilesPanelProps> = ({
  client,
  catalog,
  config,
  activeLayer,
  selectedKeyIndex,
  keymapBlocked = false,
  rules,
  onChangeRules,
  onChangeKeymap,
  onChangeAppearance,
  onImportBundle,
  onExportBundle,
  platform,
}) => {
  const canListApps = platform === undefined || platform === "macos";
  const [matcher, setMatcher] = useState("");
  const [profileIdx, setProfileIdx] = useState(1);
  const [msg, setMsg] = useState<string | null>(null);

  const [showAppPicker, setShowAppPicker] = useState(false);
  const [installedApps, setInstalledApps] = useState<InstalledAppInfo[]>([]);
  const [appQuery, setAppQuery] = useState("");

  const packs = useMemo(() => catalog?.shortcutPacks ?? [], [catalog?.shortcutPacks]);
  const [selectedPack, setSelectedPack] = useState<string>("");
  const [packSearch, setPackSearch] = useState("");
  const [packRows, setPackRows] = useState<ShortcutPackRow[]>([]);
  const [packQuery, setPackQuery] = useState("");
  const [packPlatform, setPackPlatform] = useState<"mac" | "win">("mac");

  useEffect(() => {
    if (!selectedPack && packs.length > 0) setSelectedPack(packs[0].id);
  }, [packs, selectedPack]);

  useEffect(() => {
    if (!selectedPack) return;
    let cancelled = false;
    void client
      .getShortcutPack(selectedPack)
      .then((res) => {
        if (cancelled) return;
        setPackRows(res.rows);
        const hasMac = res.rows.some((r) => Boolean(r.macKeyCode));
        const hasWin = res.rows.some((r) => Boolean(r.winKeyCode));
        if (!hasMac && hasWin) {
          setPackPlatform("win");
        }
      })
      .catch(() => {
        if (!cancelled) setPackRows([]);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedPack, client]);

  const addRule = () => {
    if (!matcher.trim()) return;
    onChangeRules([...rules, { matcher: matcher.trim(), profile_index: profileIdx, language_code: null }]);
    setMatcher("");
  };

  const applyShortcutRowToSelectedKey = (row: ShortcutPackRow) => {
    if (keymapBlocked) return;
    const kc = packPlatform === "mac" ? row.macKeyCode : row.winKeyCode;
    const mods = packPlatform === "mac" ? row.macModifiers : row.winModifiers;
    const hasSecond = packPlatform === "mac" ? row.macHasSecondKey : row.winHasSecondKey;
    if (!kc || hasSecond) return;

    const layout = config.keymap.selectedFrameLayoutType;
    const kmData = config.keymap.keymaps[layout];
    const targetKey = selectedKeyIndex ?? 0;
    if (!kmData) return;

    const nextLayers = kmData.layers.map((l, idx) => {
      if (idx !== activeLayer) return l;
      const nextActions = [...l.keyActions];
      while (nextActions.length <= targetKey) {
        nextActions.push({ keyCode: "KC_TRANSPARENT", type: 0 });
      }
      nextActions[targetKey] = {
        type: 0,
        keyCode: kc,
        modifiers: mods > 0 ? mods : null,
      };
      return { ...l, keyActions: nextActions };
    });
    onChangeKeymap({
      ...config.keymap,
      keymaps: { ...config.keymap.keymaps, [layout]: { ...kmData, layers: nextLayers } },
    });

    const theme =
      config.appearance.selectedId !== "default"
        ? (config.appearance.customThemes.find((t) => t.id === config.appearance.selectedId) ??
          config.appearance.defaultTheme)
        : config.appearance.defaultTheme;
    const stylemaps = theme.stylemaps ?? {};
    const styleLayer = stylemaps[String(activeLayer)] ?? { styles: {} };
    const stylesMap = styleLayer.styles ?? {};
    onChangeAppearance({
      ...theme,
      stylemaps: {
        ...stylemaps,
        [String(activeLayer)]: {
          ...styleLayer,
          styles: {
            ...stylesMap,
            [String(targetKey)]: {
              ...(stylesMap[String(targetKey)] ?? {}),
              label: row.label.slice(0, 12),
            },
          },
        },
      },
    });
    const targetName = humanKeyName(config, targetKey);
    setMsg(`Applied "${row.label}" (${kc}) to ${targetName} (Layer ${activeLayer})`);
  };

  const q = packQuery.trim().toLowerCase();
  // Each row keeps its position in the loaded pack, which identifies it while the filter changes.
  const filteredPackRows = packRows
    .map((row, packIndex) => ({ row, packIndex }))
    .filter(
      ({ row: r }) =>
        !q ||
        r.label.toLowerCase().includes(q) ||
        r.context.toLowerCase().includes(q) ||
        r.shortcut.toLowerCase().includes(q),
    );
  const filteredPacks = packs.filter(
    (p) => !packSearch.trim() || p.name.toLowerCase().includes(packSearch.trim().toLowerCase()),
  );
  const profileList = config.profiles ?? [];
  const targetKeyName = humanKeyName(config, selectedKeyIndex ?? 0);

  return (
    <div className="shortcuts-studio">
      {/* Left rail: application shortcut packs */}
      <aside className="studio-rail-left" aria-label="Shortcut Applications">
        <span className="rail-title">APPLICATIONS ({packs.length})</span>
        <input
          type="text"
          className="search-input"
          style={{ marginBottom: 8 }}
          placeholder="Filter apps..."
          value={packSearch}
          onChange={(e) => setPackSearch(e.target.value)}
        />
        <div className="theme-rail-list" style={{ overflowY: "auto", flex: 1 }}>
          {filteredPacks.map((p) => (
            <button
              key={p.id}
              type="button"
              className={`theme-rail-item ${selectedPack === p.id ? "active" : ""}`}
              onClick={() => setSelectedPack(p.id)}
            >
              <span className="theme-rail-meta">
                <strong>{p.name}</strong>
                <small>{p.rowCount} shortcuts</small>
              </span>
            </button>
          ))}
        </div>
      </aside>

      {/* Center: shortcuts of the selected pack */}
      <section className="card" style={{ display: "flex", flexDirection: "column", minHeight: 0, height: "100%" }}>
        <div className="toolbar-between" style={{ marginBottom: 10 }}>
          <h3>
            {packs.find((p) => p.id === selectedPack)?.name ?? "Shortcut Pack"} · Target: {targetKeyName} (L
            {activeLayer})
          </h3>
          <div className="btn-group">
            <button
              type="button"
              className={`btn-secondary ${packPlatform === "mac" ? "active" : ""}`}
              onClick={() => setPackPlatform("mac")}
            >
              macOS Bindings
            </button>
            <button
              type="button"
              className={`btn-secondary ${packPlatform === "win" ? "active" : ""}`}
              onClick={() => setPackPlatform("win")}
            >
              Windows Bindings
            </button>
          </div>
        </div>

        <label className="field" style={{ marginBottom: 10 }}>
          <span className="sr-only">Filter Shortcuts</span>
          <input
            type="text"
            aria-label="Filter Shortcuts"
            placeholder="Search action or context..."
            value={packQuery}
            onChange={(e) => setPackQuery(e.target.value)}
          />
        </label>

        <div className="shortcut-table-wrap" style={{ flex: 1, maxHeight: "none" }}>
          {filteredPackRows.slice(0, 120).map(({ row: r, packIndex }) => {
            const kc = packPlatform === "mac" ? r.macKeyCode : r.winKeyCode;
            const disp = packPlatform === "mac" ? r.macString : r.winString;
            const isChord = packPlatform === "mac" ? r.macHasSecondKey : r.winHasSecondKey;
            const canApply = Boolean(kc) && !isChord;
            const modPrefix =
              packPlatform === "mac" && !disp && kc
                ? `${(r.macModifiers & 1) !== 0 ? "⌃" : ""}${(r.macModifiers & 4) !== 0 ? "⌥" : ""}${(r.macModifiers & 2) !== 0 ? "⇧" : ""}${(r.macModifiers & 8) !== 0 ? "⌘" : ""}`
                : "";
            const prettyBinding =
              packPlatform === "mac"
                ? (disp || (kc ? `${modPrefix}${kc.replace(/^KC_/, "")}` : "Unmapped"))
                    .replace(/\bCmd\s*\+\s*/gi, "⌘")
                    .replace(/\bShift\s*\+\s*/gi, "⇧")
                    .replace(/\b(Opt|Option|Alt)\s*\+\s*/gi, "⌥")
                    .replace(/\b(Ctrl|Control)\s*\+\s*/gi, "⌃")
                    .replace(/\bEnter\b/gi, "↩")
                : disp || kc?.replace(/^KC_/, "") || "Unmapped";
            return (
              <div key={packIndex} className="shortcut-row">
                <div>
                  <strong>{r.label}</strong>
                  <small className="muted" style={{ display: "block" }}>
                    {r.context || "General"} · {prettyBinding}
                    {isChord ? " (2-key chord)" : ""}
                  </small>
                </div>
                <button
                  type="button"
                  className="btn-secondary"
                  disabled={!canApply || keymapBlocked}
                  onClick={() => applyShortcutRowToSelectedKey(r)}
                >
                  {isChord ? "Chord" : canApply ? `Apply → ${targetKeyName}` : "N/A"}
                </button>
              </div>
            );
          })}
        </div>
      </section>

      {/* Right rail: auto-switch rules, bundles and icons */}
      <aside className="studio-rail-right" aria-label="Automation and Archives">
        <div className="rail-header">
          <span className="rail-title">AUTO-SWITCH RULES</span>
          {canListApps && (
            <button
              type="button"
              className="btn-icon"
              onClick={async () => {
                setShowAppPicker(true);
                try {
                  const apps = await client.getInstalledApps();
                  setInstalledApps(apps);
                } catch (e) {
                  setMsg(String(e));
                }
              }}
            >
              Pick App
            </button>
          )}
        </div>
        <label className="field">
          <span>Bundle ID / Process Substring</span>
          <input
            type="text"
            value={matcher}
            placeholder="com.microsoft.VSCode"
            onChange={(e) => setMatcher(e.target.value)}
          />
        </label>
        <label className="field">
          <span>Target Profile</span>
          <input
            type="number"
            aria-label="Target Profile Index"
            min={0}
            max={63}
            value={profileIdx}
            onChange={(e) => setProfileIdx(Number(e.target.value))}
          />
          {profileList[profileIdx] && <small className="muted">Maps to: {profileList[profileIdx].name}</small>}
        </label>
        <button type="button" className="btn-primary" onClick={addRule}>
          Add App Rule
        </button>

        <ul className="rule-list">
          {rules.map((r, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: rules have no id and may repeat a matcher; a row holds no local state, so positional keys render correctly after a removal.
            <li key={`${r.matcher}-${i}`}>
              <span style={{ fontSize: 11, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>
                <code>{r.matcher}</code> → {profileList[r.profile_index]?.name ?? `Profile #${r.profile_index}`}
              </span>
              <button
                type="button"
                className="btn-icon"
                aria-label={`Remove rule ${r.matcher}`}
                onClick={() => onChangeRules(rules.filter((_, idx) => idx !== i))}
              >
                ×
              </button>
            </li>
          ))}
        </ul>

        <div className="rail-divider" />
        <span className="rail-title">BUNDLES &amp; CUSTOM ICONS</span>

        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <button
            type="button"
            className="btn-secondary"
            style={{ width: "100%" }}
            onClick={async () => {
              const outPath = await client.pickSaveFile("Export OpenFlux Bundle", "openflux-profile.openflux", [
                "openflux",
              ]);
              if (!outPath?.trim()) return;
              try {
                const res = await onExportBundle(outPath.trim());
                setMsg(res.message);
              } catch (err) {
                setMsg(String(err));
              }
            }}
          >
            Export OpenFlux Bundle...
          </button>

          <button
            type="button"
            className="btn-secondary"
            style={{ width: "100%" }}
            onClick={async () => {
              const file = await client.pickFile("Import an OpenFlux bundle or a .flux file", ["openflux", "flux"]);
              if (!file?.trim()) return;
              try {
                const res = await onImportBundle(file.trim());
                setMsg(`Imported: ${res.profiles.join(", ") || res.message}`);
              } catch (err) {
                setMsg(String(err));
              }
            }}
          >
            Import Bundle or .flux File...
          </button>

          <button
            type="button"
            className="btn-secondary"
            style={{ width: "100%" }}
            onClick={async () => {
              const file = await client.pickFile("Add Custom Key Icon", ["png", "jpg", "jpeg", "bmp", "svg"]);
              if (!file?.trim()) return;
              try {
                const res = await client.addIcon(file.trim());
                setMsg(`Imported icon ID: ${res.iconPaths.join(", ")}`);
              } catch (err) {
                setMsg(String(err));
              }
            }}
          >
            Add Custom Key Icon...
          </button>
        </div>
        {msg && <p className="status-msg">{msg}</p>}
      </aside>

      {showAppPicker && (
        <div className="modal-backdrop" role="dialog" aria-label="App Picker">
          <div className="modal-card modal-wide">
            <div className="toolbar-between">
              <h3>Select Application ({installedApps.length})</h3>
              <button type="button" className="btn-secondary" onClick={() => setShowAppPicker(false)}>
                Close
              </button>
            </div>
            <input
              type="text"
              className="search-input"
              placeholder="Filter installed apps…"
              value={appQuery}
              onChange={(e) => setAppQuery(e.target.value)}
            />
            <div className="shortcut-table-wrap">
              {installedApps
                .filter(
                  (a) =>
                    a.name.toLowerCase().includes(appQuery.trim().toLowerCase()) ||
                    a.bundleId.toLowerCase().includes(appQuery.trim().toLowerCase()),
                )
                .slice(0, 60)
                .map((a) => (
                  <div key={a.bundleId} className="shortcut-row">
                    <div>
                      <strong>{a.name}</strong>
                      <small className="muted" style={{ display: "block" }}>
                        {a.bundleId}
                      </small>
                    </div>
                    <button
                      type="button"
                      className="btn-secondary"
                      onClick={() => {
                        setMatcher(a.bundleId);
                        setShowAppPicker(false);
                      }}
                    >
                      Use Bundle ID
                    </button>
                  </div>
                ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
