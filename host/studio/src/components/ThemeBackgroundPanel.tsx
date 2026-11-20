import type React from "react";
import { useEffect, useRef, useState } from "react";
import type { AerialVideoItem, DaemonClient } from "../api/daemonClient";
import { KEYBOARD_MEDIA_LIMIT_MIB } from "../api/daemonClient";
import { useMountedRef } from "../hooks/useMountedRef";
import type { AppearanceTheme, AssetCatalog, KeyEffect, OpenFluxConfig } from "../types/openflux";
import { argbToHexColor, hexColorToArgb } from "../types/openflux";
import { FontBrowserModal, PresetBrowserModal } from "./AssetBrowsers";
import { describeImport, importErrorText } from "./mediaImportText";

interface ThemeBackgroundPanelProps {
  client: DaemonClient;
  catalog: AssetCatalog | null;
  onRefreshCatalog: () => void;
  config: OpenFluxConfig;
  resetEpoch?: number;
  previewNode?: React.ReactNode;
  onChangeAppearance: (theme: AppearanceTheme) => void;
  onSelectTheme?: (id: string) => void;
  onCreateCustomTheme?: (theme: AppearanceTheme) => void;
  onPatchDefaultTheme?: (
    partial: Partial<AppearanceTheme>,
    expectedResetEpoch?: number,
    targetThemeId?: string,
  ) => void;
}

const FONT_FAMILIES = [
  "Montserrat",
  "Inter",
  "InterTight",
  "RobotoMono",
  "Rubik",
  "NotoSans",
  "PlayfairDisplay",
  "PressStart2P",
  "BebasNeue",
];

const KEY_EFFECTS: { value: KeyEffect; label: string; desc: string }[] = [
  { value: "none", label: "None", desc: "Clean legends with no press highlight" },
  { value: "glowing_rings", label: "Glowing Rings", desc: "Soft luminous halo around active keys" },
  { value: "solid_highlight", label: "Solid Highlight", desc: "Crisp tinted fill on key press" },
  { value: "ripple", label: "Ripple Pulse", desc: "Expanding ring pulse fading on release" },
  { value: "autofocus_lock", label: "Autofocus Lock", desc: "Camera reticle brackets snapping onto the key" },
];

const RING_SWATCHES = [
  "#66ccff",
  "#2dd4bf",
  "#34d399",
  "#a3e635",
  "#facc15",
  "#fb923c",
  "#f87171",
  "#f472b6",
  "#c084fc",
  "#818cf8",
  "#ffffff",
  "#94a3b8",
];

type ThemeSubTab = "backgrounds" | "key_effects" | "typography";

const PROCEDURAL_SHADERS: {
  id: string;
  file: string;
  name: string;
  desc: string;
  previewBg: string;
}[] = [
  {
    id: "aurora_drift",
    file: "aurora_drift.frag",
    name: "Aurora Drift",
    desc: "Slow ribbons of green and violet light",
    previewBg: "linear-gradient(135deg, #04121f 0%, #10b981 45%, #8b5cf6 100%)",
  },
  {
    id: "cobalt_waves",
    file: "cobalt_waves.frag",
    name: "Cobalt Waves",
    desc: "Layered deep blue waves",
    previewBg: "linear-gradient(135deg, #030a1c 0%, #1d4ed8 55%, #60a5fa 100%)",
  },
  {
    id: "horizon_split",
    file: "horizon_split.frag",
    name: "Horizon Split",
    desc: "Cool sky above a warm horizon",
    previewBg: "linear-gradient(180deg, #0c1a3a 0%, #2563eb 48%, #f59e0b 52%, #7c2d12 100%)",
  },
  {
    id: "tidal_caustics",
    file: "tidal_caustics.frag",
    name: "Tidal Caustics",
    desc: "Shifting underwater light patterns",
    previewBg: "linear-gradient(135deg, #042f2e 0%, #0f766e 55%, #5eead4 100%)",
  },
  {
    id: "perimeter_glow",
    file: "perimeter_glow.frag",
    name: "Perimeter Glow",
    desc: "Soft light moving around the display edges",
    previewBg: "radial-gradient(circle, #0b0d12 55%, #f59e0b 100%)",
  },
];

export const ThemeBackgroundPanel: React.FC<ThemeBackgroundPanelProps> = ({
  client,
  catalog,
  onRefreshCatalog,
  config,
  resetEpoch = 0,
  previewNode,
  onChangeAppearance,
  onSelectTheme,
  onCreateCustomTheme,
  onPatchDefaultTheme,
}) => {
  const theme =
    config.appearance.selectedId !== "default"
      ? (config.appearance.customThemes.find((t) => t.id === config.appearance.selectedId) ??
        config.appearance.defaultTheme)
      : config.appearance.defaultTheme;
  const [subTab, setSubTab] = useState<ThemeSubTab>("backgrounds");
  const [bgSourceTab, setBgSourceTab] = useState<"library" | "shaders" | "aerials">("library");
  const [aerials, setAerials] = useState<AerialVideoItem[]>([]);
  const [aerialQuery, setAerialQuery] = useState("");
  const [loopReverse, setLoopReverse] = useState(false);
  const [loadingAerials, setLoadingAerials] = useState(false);
  const [aerialErr, setAerialErr] = useState<string | null>(null);
  const [showFonts, setShowFonts] = useState(false);
  const [showPresets, setShowPresets] = useState(false);
  const [mediaMsg, setMediaMsg] = useState<string | null>(null);
  const [importingMedia, setImportingMedia] = useState(false);
  const [mediaThumbs, setMediaThumbs] = useState<Record<string, string>>({});

  const patchTheme = (partial: Partial<AppearanceTheme>) => onChangeAppearance({ ...theme, ...partial });

  const fontsList = catalog?.fonts.length ? catalog.fonts.map((f) => f.family) : FONT_FAMILIES;
  const availableFonts = fontsList.includes(theme.fontFamily) ? fontsList : [theme.fontFamily, ...fontsList];

  // Names whose thumbnail is loaded or loading. A failed load is removed so a later catalog can retry it.
  const requestedThumbsRef = useRef<Set<string>>(new Set());
  const mountedRef = useMountedRef();

  // Every tile's thumbnail is loaded, a few at a time so a large library doesn't flood the daemon.
  const media = catalog?.media;
  useEffect(() => {
    if (!media) return;
    const queue = media.flatMap((m) => {
      const cat = (m as { category?: string }).category || "media_themes";
      const source = m.thumbnail
        ? { cat: "thumbnails", name: m.thumbnail }
        : m.kind === "image"
          ? { cat, name: m.name }
          : null;
      if (!source || requestedThumbsRef.current.has(m.name)) return [];
      requestedThumbsRef.current.add(m.name);
      return [{ key: m.name, ...source }];
    });
    let stopped = false;
    const next = async (): Promise<void> => {
      for (let item = queue.shift(); item && !stopped; item = queue.shift()) {
        const { key, cat, name } = item;
        try {
          const url = await client.getAssetDataUrl(cat, name);
          if (mountedRef.current) setMediaThumbs((prev) => ({ ...prev, [key]: url }));
        } catch {
          requestedThumbsRef.current.delete(key);
        }
      }
    };
    for (let i = 0; i < 4; i++) void next();
    return () => {
      stopped = true;
      // Names never started can be requested again by the next catalog.
      for (const item of queue) requestedThumbsRef.current.delete(item.key);
    };
  }, [media, client, mountedRef]);

  const allThemes: { id: string; name: string; badge: string; ringHex: string }[] = [
    {
      id: "default",
      name: config.appearance.defaultTheme.name,
      badge: "Base Theme",
      ringHex: argbToHexColor(config.appearance.defaultTheme.ringGlowColor).toLowerCase(),
    },
    ...(config.appearance.customThemes ?? []).map((ct) => ({
      id: ct.id,
      name: ct.name,
      badge: "Custom Theme",
      ringHex: argbToHexColor(ct.ringGlowColor).toLowerCase(),
    })),
  ];

  const curRingHex = argbToHexColor(theme.ringGlowColor).toLowerCase();

  return (
    <div className="themes-studio">
      {/* Left Rail: Theme List + Presets */}
      <aside className="studio-rail-left" aria-label="Themes">
        <div className="rail-header">
          <span className="rail-title">THEMES</span>
          <div className="rail-actions-row">
            {onCreateCustomTheme && (
              <button
                type="button"
                className="btn-icon"
                title="Duplicate into new custom theme"
                onClick={() => {
                  const id = `theme_${Date.now().toString(36)}`;
                  onCreateCustomTheme({
                    ...structuredClone(theme),
                    id,
                    name: `${theme.name} Copy`,
                  });
                }}
              >
                + Save
              </button>
            )}
            <button type="button" className="btn-icon" onClick={() => setShowPresets(true)}>
              Presets
            </button>
          </div>
        </div>

        {onSelectTheme && (
          <label className="field rail-select-wrap" style={{ marginBottom: 4 }}>
            <span className="sr-only">Editing Theme</span>
            <select
              aria-label="Editing Theme"
              value={config.appearance.selectedId}
              onChange={(e) => onSelectTheme(e.target.value)}
            >
              <option value="default">Default: {config.appearance.defaultTheme.name}</option>
              {(config.appearance.customThemes ?? []).map((ct) => (
                <option key={ct.id} value={ct.id}>
                  Custom: {ct.name}
                </option>
              ))}
            </select>
          </label>
        )}

        <div className="theme-rail-list">
          {allThemes.map((item) => {
            const active = (config.appearance.selectedId || "default") === item.id;
            return (
              <button
                key={item.id}
                type="button"
                className={`theme-rail-item ${active ? "active" : ""}`}
                onClick={() => onSelectTheme?.(item.id)}
              >
                <span className="theme-rail-swatch" style={{ background: item.ringHex }} />
                <span className="theme-rail-meta">
                  <strong>{item.name}</strong>
                  <small>{item.badge}</small>
                </span>
              </button>
            );
          })}
        </div>

        <div className="rail-divider" />
        <label className="field" style={{ marginBottom: 0 }}>
          <span>Theme Name</span>
          <input type="text" value={theme.name} onChange={(e) => patchTheme({ name: e.target.value })} />
        </label>
      </aside>

      {/* Center: live display preview and the theme sub-tabs */}
      <section className="studio-center">
        {previewNode && <div className="studio-hero-preview">{previewNode}</div>}

        <div className="subtabs-bar" role="tablist" aria-label="Theme Sections">
          {(
            [
              ["backgrounds", "Backgrounds"],
              ["key_effects", "Key Effects"],
              ["typography", "Typography & Styles"],
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={subTab === id}
              className={`subtab-btn ${subTab === id ? "active" : ""}`}
              onClick={() => setSubTab(id)}
            >
              {label}
            </button>
          ))}
        </div>

        <div className="subtab-content">
          {subTab === "backgrounds" && (
            <div className="bg-gallery-pane">
              <div className="toolbar-between" style={{ marginBottom: 8, gap: 8, flexWrap: "wrap" }}>
                <div
                  style={{
                    display: "inline-flex",
                    background: "var(--bg-input)",
                    padding: 3,
                    borderRadius: 8,
                    gap: 2,
                    border: "1px solid var(--border-subtle)",
                  }}
                >
                  <button
                    type="button"
                    style={{ padding: "5px 11px", whiteSpace: "nowrap" }}
                    className={`seg-btn ${bgSourceTab === "library" ? "active" : ""}`}
                    onClick={() => setBgSourceTab("library")}
                  >
                    My Library
                  </button>
                  <button
                    type="button"
                    style={{ padding: "5px 11px", whiteSpace: "nowrap" }}
                    className={`seg-btn ${bgSourceTab === "shaders" ? "active" : ""}`}
                    onClick={() => setBgSourceTab("shaders")}
                  >
                    Animated backgrounds
                  </button>
                  <button
                    type="button"
                    style={{ padding: "5px 11px", whiteSpace: "nowrap" }}
                    className={`seg-btn ${bgSourceTab === "aerials" ? "active" : ""}`}
                    onClick={async () => {
                      setBgSourceTab("aerials");
                      if (aerials.length === 0 && !loadingAerials) {
                        setLoadingAerials(true);
                        setAerialErr(null);
                        try {
                          const list = await client.getAerialVideos();
                          setAerials(list);
                        } catch (e) {
                          setAerialErr(String(e));
                        } finally {
                          setLoadingAerials(false);
                        }
                      }
                    }}
                  >
                    Aerial Videos
                  </button>
                </div>
                <div className="btn-group" style={{ alignItems: "center", gap: 10 }}>
                  <label
                    style={{ display: "inline-flex", alignItems: "center", gap: 5, fontSize: 12, cursor: "pointer" }}
                  >
                    <input type="checkbox" checked={loopReverse} onChange={(e) => setLoopReverse(e.target.checked)} />
                    Ping-Pong Loop (Forward → Reverse)
                  </label>
                  {bgSourceTab === "library" && (
                    <button
                      type="button"
                      className="btn-secondary"
                      disabled={importingMedia}
                      onClick={async () => {
                        const picked = await client.pickFile("Import Background Image or Video", [
                          "png",
                          "jpg",
                          "jpeg",
                          "webp",
                          "gif",
                          "mp4",
                          "mov",
                          "m4v",
                          "webm",
                          "mkv",
                        ]);
                        if (!picked?.trim()) return;
                        const startEpoch = resetEpoch;
                        const startThemeId = config.appearance.selectedId || theme.id;
                        setImportingMedia(true);
                        setMediaMsg(null);
                        try {
                          const res = await client.importMedia(picked.trim(), { loopReverse });
                          if (onPatchDefaultTheme) {
                            onPatchDefaultTheme({ backgroundImagePath: res.assetPath }, startEpoch, startThemeId);
                          } else {
                            patchTheme({ backgroundImagePath: res.assetPath });
                          }
                          setMediaMsg(
                            describeImport(
                              res.transcoded ? "Converted and imported background media" : "Imported background media",
                              res,
                            ),
                          );
                          onRefreshCatalog();
                        } catch (e) {
                          setMediaMsg(`Couldn't import the file: ${importErrorText(e)}`);
                        } finally {
                          setImportingMedia(false);
                        }
                      }}
                    >
                      {importingMedia ? "Importing..." : "+ Import Image / Video"}
                    </button>
                  )}
                </div>
              </div>
              {bgSourceTab !== "shaders" && (
                <p className="muted" style={{ fontSize: 11 }}>
                  Videos keep their full length. To fit the keyboard's {KEYBOARD_MEDIA_LIMIT_MIB} MiB media limit,
                  OpenFlux lowers the bitrate or resolution; a video that can't fit at acceptable quality isn't
                  imported.
                </p>
              )}
              {mediaMsg && <p className="status-msg">{mediaMsg}</p>}

              {bgSourceTab === "shaders" ? (
                <div>
                  {theme.backgroundImagePath.endsWith(".frag") && (
                    <div className="row-2" style={{ marginBottom: 10 }}>
                      <label className="field">
                        <span>Shader Hue ({Math.round(theme.backgroundShaderParams?.hue ?? 0)}°)</span>
                        <input
                          type="range"
                          min={-180}
                          max={180}
                          step={5}
                          value={theme.backgroundShaderParams?.hue ?? 0}
                          onChange={(e) =>
                            patchTheme({
                              backgroundShaderParams: {
                                ...(theme.backgroundShaderParams ?? {}),
                                hue: Number(e.target.value),
                              },
                            })
                          }
                        />
                      </label>
                      <label className="field">
                        <span>Shader Speed ({(theme.backgroundShaderParams?.speed ?? 1).toFixed(2)}×)</span>
                        <input
                          type="range"
                          min={0.25}
                          max={4.0}
                          step={0.25}
                          value={theme.backgroundShaderParams?.speed ?? 1}
                          onChange={(e) =>
                            patchTheme({
                              backgroundShaderParams: {
                                ...(theme.backgroundShaderParams ?? {}),
                                speed: Number(e.target.value),
                              },
                            })
                          }
                        />
                      </label>
                    </div>
                  )}
                  <fieldset className="media-grid" aria-label="Animated Backgrounds">
                    {PROCEDURAL_SHADERS.map((sh) => {
                      const active = theme.backgroundImagePath === sh.file;
                      return (
                        <button
                          key={sh.id}
                          type="button"
                          className={`media-tile ${active ? "selected" : ""}`}
                          title={sh.desc}
                          onClick={() =>
                            patchTheme({
                              backgroundImagePath: sh.file,
                              backgroundShaderParams: theme.backgroundShaderParams ?? { hue: 0, speed: 1 },
                            })
                          }
                        >
                          <div className="media-tile-blank" style={{ background: sh.previewBg }}>
                            Animated background
                          </div>
                          <span className="media-tile-label">{sh.name}</span>
                        </button>
                      );
                    })}
                  </fieldset>
                </div>
              ) : bgSourceTab === "aerials" ? (
                <div>
                  <div className="toolbar-between" style={{ marginBottom: 8, gap: 8 }}>
                    <input
                      type="text"
                      className="search-input"
                      style={{ flex: 1, marginBottom: 0 }}
                      placeholder="Search Apple Aerial videos (Tokyo, Aurora, Yosemite...)"
                      value={aerialQuery}
                      onChange={(e) => setAerialQuery(e.target.value)}
                    />
                  </div>
                  {aerialErr && <p className="status-msg">{aerialErr}</p>}
                  {loadingAerials && (
                    <p className="muted" style={{ fontSize: 12 }}>
                      Loading Aerial Videos catalog...
                    </p>
                  )}
                  <fieldset className="media-grid" aria-label="Aerial Videos Catalog">
                    {aerials
                      .filter(
                        (a) => !aerialQuery.trim() || a.title.toLowerCase().includes(aerialQuery.trim().toLowerCase()),
                      )
                      .slice(0, 60)
                      .map((a) => (
                        <button
                          key={a.id}
                          type="button"
                          disabled={importingMedia}
                          className="media-tile"
                          title={`Download & apply ${a.title}`}
                          onClick={async () => {
                            const startEpoch = resetEpoch;
                            const startThemeId = config.appearance.selectedId || theme.id;
                            setImportingMedia(true);
                            setMediaMsg(`Downloading & transcoding "${a.title}"...`);
                            try {
                              const res = await client.importAerialVideo(a.videoUrl, a.title, { loopReverse });
                              if (onPatchDefaultTheme) {
                                onPatchDefaultTheme({ backgroundImagePath: res.assetPath }, startEpoch, startThemeId);
                              } else {
                                patchTheme({ backgroundImagePath: res.assetPath });
                              }
                              setMediaMsg(describeImport(`Applied Aerial video: ${a.title}`, res));
                              onRefreshCatalog();
                            } catch (e) {
                              setMediaMsg(`Couldn't import "${a.title}": ${importErrorText(e)}`);
                            } finally {
                              setImportingMedia(false);
                            }
                          }}
                        >
                          <img src={a.thumbnailUrl} alt={a.title} loading="lazy" />
                          <span className="media-badge">{a.quality}</span>
                          <span className="media-tile-label">{a.title}</span>
                        </button>
                      ))}
                  </fieldset>
                </div>
              ) : (
                <fieldset className="media-grid" aria-label="Background Media Library">
                  <button
                    type="button"
                    className={`media-tile ${theme.backgroundImagePath === "" ? "selected" : ""}`}
                    onClick={() => patchTheme({ backgroundImagePath: "" })}
                  >
                    <div className="media-tile-blank">Solid Gradient</div>
                    <span className="media-tile-label">Default Gradient</span>
                  </button>
                  {(catalog?.media ?? []).map((m, idx) => {
                    const isVideo = m.kind === "video";
                    const niceName =
                      (m as { category?: string }).category === "default_backgrounds"
                        ? m.name
                            .replace(/\.(png|jpg|jpeg)$/i, "")
                            .replace(/_/g, " ")
                            .replace(/\b\w/g, (c) => c.toUpperCase())
                        : isVideo
                          ? `Animated Loop ${idx + 1}`
                          : `Wallpaper ${idx + 1}`;
                    return (
                      <button
                        key={m.name}
                        type="button"
                        className={`media-tile ${theme.backgroundImagePath === m.name ? "selected" : ""}`}
                        onClick={() => patchTheme({ backgroundImagePath: m.name })}
                        title={m.name}
                      >
                        {mediaThumbs[m.name] ? (
                          <img src={mediaThumbs[m.name]} alt={m.name} />
                        ) : (
                          <div className={`media-tile-blank ${isVideo ? "media-tile-video" : ""}`}>
                            {isVideo ? "▶ Loop Video" : m.kind.toUpperCase()}
                          </div>
                        )}
                        {isVideo && <span className="media-badge">MP4</span>}
                        <span className="media-tile-label">{niceName}</span>
                      </button>
                    );
                  })}
                </fieldset>
              )}
            </div>
          )}

          {subTab === "key_effects" && (
            <div className="effects-pane">
              <div className="effect-cards-row">
                {KEY_EFFECTS.map((fx) => {
                  const sel = theme.selectedKeyEffect === fx.value;
                  return (
                    <button
                      key={fx.value}
                      type="button"
                      className={`effect-card ${sel ? "selected" : ""}`}
                      onClick={() => patchTheme({ selectedKeyEffect: fx.value })}
                    >
                      {sel && (
                        <span className="effect-check" aria-hidden="true">
                          ✓
                        </span>
                      )}
                      <div className={`effect-preview effect-${fx.value}`}>
                        <span />
                      </div>
                      <strong>{fx.label}</strong>
                      <small>{fx.desc}</small>
                    </button>
                  );
                })}
              </div>
              <div className="effect-toggles-strip">
                <label className="toggle-pill">
                  <input
                    type="checkbox"
                    checked={theme.blendingModePreset === "dropShadow" || theme.blendingModePreset === "lightGlow"}
                    onChange={(e) =>
                      patchTheme({
                        blendingModePreset: e.target.checked
                          ? theme.blendingModePreset === "lightGlow"
                            ? "lightGlow"
                            : "dropShadow"
                          : "off",
                      })
                    }
                  />
                  <span>Drop Shadow under Key Legends</span>
                </label>
                <span className="muted">
                  Click any key on the preview above to test {theme.selectedKeyEffect.replace("_", " ")} live.
                </span>
              </div>
            </div>
          )}

          {subTab === "typography" && (
            <div className="typo-pane">
              <div className="row-3">
                <label className="field">
                  <span>Font Family</span>
                  <select value={theme.fontFamily} onChange={(e) => patchTheme({ fontFamily: e.target.value })}>
                    {availableFonts.map((f) => (
                      <option key={f} value={f}>
                        {f}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="field">
                  <span>Weight ({theme.fontWeight})</span>
                  <input
                    type="range"
                    min={100}
                    max={900}
                    step={100}
                    value={theme.fontWeight}
                    onChange={(e) => patchTheme({ fontWeight: Number(e.target.value) })}
                  />
                </label>
                <div className="field">
                  <span>Full Font Library</span>
                  <button type="button" className="btn-secondary" onClick={() => setShowFonts(true)}>
                    Browse All Fonts ({availableFonts.length})
                  </button>
                </div>
              </div>
              <div className="checkbox-row">
                <label>
                  <input
                    type="checkbox"
                    checked={theme.fontItalic}
                    onChange={(e) => patchTheme({ fontItalic: e.target.checked })}
                  />
                  Italic
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={theme.retintIcons}
                    onChange={(e) => patchTheme({ retintIcons: e.target.checked })}
                  />
                  Retint Key Icons
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={theme.retintModules}
                    onChange={(e) => patchTheme({ retintModules: e.target.checked })}
                  />
                  Retint Mosaic Modules
                </label>
              </div>
              <div className="rail-divider" style={{ margin: "12px 0" }} />
              <div className="row-2">
                <label className="field">
                  <span>Lock Key Indicator (Caps / Num / Scroll)</span>
                  <select
                    value={theme.lockIndicatorStyle ?? "dot"}
                    onChange={(e) => patchTheme({ lockIndicatorStyle: e.target.value as "off" | "dot" | "highlight" })}
                  >
                    <option value="dot">Status Dot (Default)</option>
                    <option value="highlight">Keycap Highlight</option>
                    <option value="off">Off</option>
                  </select>
                </label>
                <div className="field">
                  <span>Lock Indicator Color</span>
                  <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                    <input
                      type="color"
                      value={argbToHexColor(theme.lockIndicatorColor ?? theme.ringGlowColor)}
                      onChange={(e) => patchTheme({ lockIndicatorColor: hexColorToArgb(e.target.value) })}
                    />
                    {theme.lockIndicatorColor != null && (
                      <button
                        type="button"
                        className="btn-secondary"
                        style={{ padding: "4px 8px", fontSize: 11 }}
                        onClick={() => patchTheme({ lockIndicatorColor: null })}
                      >
                        Reset
                      </button>
                    )}
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>
      </section>

      {/* Right Rail: Quick Background, Font & Key Effect Properties */}
      <aside className="studio-rail-right" aria-label="Theme Properties">
        <span className="rail-title">PROPERTIES</span>

        <label className="field">
          <span>Background Blur ({theme.backgroundImageBlurAmount}px)</span>
          <input
            type="range"
            min={0}
            max={20}
            step={1}
            value={theme.backgroundImageBlurAmount}
            onChange={(e) => patchTheme({ backgroundImageBlurAmount: Number(e.target.value) })}
          />
        </label>
        <label className="field">
          <span>Background Dimming ({theme.backgroundImageDimming}%)</span>
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={theme.backgroundImageDimming}
            onChange={(e) => patchTheme({ backgroundImageDimming: Number(e.target.value) })}
          />
        </label>
        <label className="field">
          <span>Module Dimming ({theme.moduleBackgroundDimming}%)</span>
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={theme.moduleBackgroundDimming}
            onChange={(e) => patchTheme({ moduleBackgroundDimming: Number(e.target.value) })}
          />
        </label>

        <div className="rail-divider" />
        <span className="rail-title">COLORS &amp; FONT</span>

        <div className="field" style={{ marginBottom: 4 }}>
          <span>Ring Glow Swatches</span>
          <fieldset className="swatch-ring" aria-label="Ring Color Swatches">
            {RING_SWATCHES.map((hex) => (
              <button
                key={hex}
                type="button"
                className={`swatch-dot ${curRingHex === hex ? "active" : ""}`}
                style={{ background: hex }}
                title={`Set Ring Color to ${hex}`}
                onClick={() => patchTheme({ ringGlowColor: hexColorToArgb(hex) })}
              />
            ))}
          </fieldset>
        </div>

        <div className="row-2" style={{ marginTop: 6 }}>
          <label className="field">
            <span>Ring Color</span>
            <input
              type="color"
              value={curRingHex}
              onChange={(e) => patchTheme({ ringGlowColor: hexColorToArgb(e.target.value) })}
            />
          </label>
          <label className="field">
            <span>Legend Color</span>
            <input
              type="color"
              value={argbToHexColor(theme.legendColor)}
              onChange={(e) => patchTheme({ legendColor: hexColorToArgb(e.target.value) })}
            />
          </label>
        </div>

        <label className="field">
          <span>Font Family</span>
          <select value={theme.fontFamily} onChange={(e) => patchTheme({ fontFamily: e.target.value })}>
            {availableFonts.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        </label>
      </aside>

      {showFonts && (
        <FontBrowserModal
          client={client}
          catalog={catalog}
          currentFamily={theme.fontFamily}
          onSelect={(family) => patchTheme({ fontFamily: family })}
          onClose={() => setShowFonts(false)}
        />
      )}
      {showPresets && (
        <PresetBrowserModal
          client={client}
          catalog={catalog}
          onApplyPreset={(p) =>
            patchTheme({
              name: p.name,
              fontFamily: p.fontFamily,
              fontWeight: p.fontWeight,
              fontItalic: p.fontItalic,
              backgroundImagePath: p.backgroundImagePath,
              backgroundImageBlurAmount: p.backgroundImageBlurAmount,
              backgroundImageDimming: p.backgroundImageDimming,
              moduleBackgroundDimming: p.moduleBackgroundDimming,
              selectedKeyEffect: p.selectedKeyEffect,
              ringGlowColor: p.ringGlowColor,
              legendColor: p.legendColor,
              blendingModePreset: p.blendingModePreset,
            })
          }
          onClose={() => setShowPresets(false)}
        />
      )}
    </div>
  );
};
