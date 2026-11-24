import type React from "react";
import { useCallback, useEffect, useState } from "react";
import {
  type StreamDeckButton,
  type StreamDeckPluginInfo,
  v2Client,
} from "../api/v2Client";

interface StreamDeckViewProps {
  pickFile?: (title: string, extensions?: string[]) => Promise<string | null>;
  pickFolder?: (title: string) => Promise<string | null>;
  onOpenKeymap: (focusKey?: number) => void;
}

const BUILTIN_ACTIONS: Array<{
  uuid: string;
  name: string;
  desc: string;
  defaultTitle: string;
  defaultColor: string;
  defaultSettings: Record<string, unknown>;
}> = [
  {
    uuid: "builtin.open_url",
    name: "Open Website / URL",
    desc: "Opens any link in your browser",
    defaultTitle: "Docs",
    defaultColor: "#60a5fa",
    defaultSettings: { url: "https://github.com" },
  },
  {
    uuid: "builtin.launch_app",
    name: "Launch / Focus App",
    desc: "Opens or switches to a macOS application",
    defaultTitle: "Spotify",
    defaultColor: "#58f29c",
    defaultSettings: { app: "Spotify" },
  },
  {
    uuid: "builtin.run_command",
    name: "Run Shell Command",
    desc: "Executes any script or CLI command",
    defaultTitle: "Run",
    defaultColor: "#f59e0b",
    defaultSettings: { command: "say 'Hello from KeyGlass'" },
  },
  {
    uuid: "builtin.toggle_command",
    name: "2-State Toggle Button",
    desc: "Toggles between On/Off commands and updates the live keycap title & color",
    defaultTitle: "Toggle",
    defaultColor: "#a78bfa",
    defaultSettings: { on_title: "ON", off_title: "OFF", on_command: "", off_command: "" },
  },
  {
    uuid: "builtin.counter",
    name: "Live Tap Counter",
    desc: "Increments a live number directly on the physical keycap",
    defaultTitle: "Count",
    defaultColor: "#38bdf8",
    defaultSettings: {},
  },
];

export const StreamDeckView: React.FC<StreamDeckViewProps> = ({
  pickFile,
  pickFolder,
  onOpenKeymap,
}) => {
  const [plugins, setPlugins] = useState<StreamDeckPluginInfo[]>([]);
  const [elgatoDetected, setElgatoDetected] = useState<StreamDeckPluginInfo[]>([]);
  const [buttons, setButtons] = useState<StreamDeckButton[]>([]);
  const [selectedBtnId, setSelectedBtnId] = useState<string | null>(null);
  const [boundByBtnId, setBoundByBtnId] = useState<Record<string, { keyIndex: number; layer: number }>>({});
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);

  const loadState = useCallback(async () => {
    try {
      const [sdRes, saRes, rkRes] = await Promise.all([
        v2Client.getStreamDeckState(),
        v2Client.getBusRetained("system/actions"),
        v2Client.getBusRetained("keyboard/resolved_keys"),
      ]);
      setPlugins(sdRes.plugins ?? []);
      setElgatoDetected(sdRes.elgato_detected ?? []);
      setButtons(sdRes.buttons ?? []);

      const slots = (saRes.messages[0]?.payload as { slots?: Record<string, { action_id: string; keycode: number }> })
        ?.slots;
      const rkKeys = (rkRes.messages[0]?.payload as { keys?: Array<{ keycode: number; source_layer: number }> })?.keys;
      if (slots && rkKeys) {
        const kcToBtn = new Map<number, string>();
        for (const entry of Object.values(slots)) {
          if (entry.action_id?.startsWith("streamdeck-bridge.")) {
            kcToBtn.set(entry.keycode, entry.action_id.slice("streamdeck-bridge.".length));
          }
        }
        const map: Record<string, { keyIndex: number; layer: number }> = {};
        rkKeys.forEach((rk, idx) => {
          const bid = kcToBtn.get(rk.keycode);
          if (bid && !map[bid]) map[bid] = { keyIndex: idx, layer: rk.source_layer ?? 0 };
        });
        setBoundByBtnId(map);
      }
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void loadState();
    const id = window.setInterval(() => void loadState(), 3000);
    return () => window.clearInterval(id);
  }, [loadState]);

  const saveButtons = async (next: StreamDeckButton[], note?: string) => {
    setButtons(next);
    try {
      await v2Client.putStreamDeckButtons(next);
      // Sync updated QMK customKeycodes to firmware so the new button shows up immediately in QMK Keymap!
      await v2Client.vialSyncActions().catch(() => {});
      await loadState();
      if (note) setStatusMsg(note);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleImportArchive = async () => {
    if (!pickFile) return;
    const p = await pickFile("Select Elgato .streamDeckPlugin Archive", ["streamDeckPlugin", "zip"]);
    if (!p) return;
    setImporting(true);
    setStatusMsg(null);
    try {
      const res = await v2Client.importStreamDeckPlugin(p);
      await loadState();
      setStatusMsg(`Imported .sdPlugin "${res.plugin?.name ?? p}"!`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setImporting(false);
    }
  };

  const handleImportFolder = async (folderPath?: string) => {
    const p = folderPath ?? (pickFolder ? await pickFolder("Select .sdPlugin Folder") : null);
    if (!p) return;
    setImporting(true);
    setStatusMsg(null);
    try {
      const res = await v2Client.importStreamDeckPlugin(p);
      await loadState();
      setStatusMsg(`Imported .sdPlugin "${res.plugin?.name ?? p}"!`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setImporting(false);
    }
  };

  const handleAddButton = (
    pluginId: string,
    actionUuid: string,
    title: string,
    color = "#5ce1ff",
    iconMedia?: string | null,
    defaultSettings: Record<string, unknown> = {},
  ) => {
    const id = `sd_${Date.now().toString(36)}`;
    const nextBtn: StreamDeckButton = {
      id,
      title,
      plugin_id: pluginId,
      action_uuid: actionUuid,
      icon_media: iconMedia ?? null,
      color,
      settings: defaultSettings,
    };
    setSelectedBtnId(id);
    void saveButtons(
      [...buttons, nextBtn],
      `Created Stream Deck button "${title}" — bind it to ANY key on ANY layer in QMK Keymap!`,
    );
  };

  const updateButton = (id: string, patch: Partial<StreamDeckButton>) => {
    const next = buttons.map((b) => (b.id === id ? { ...b, ...patch } : b));
    void saveButtons(next, "Saved Stream Deck button.");
  };

  const deleteButton = (id: string) => {
    const next = buttons.filter((b) => b.id !== id);
    if (selectedBtnId === id) setSelectedBtnId(null);
    void saveButtons(next, "Removed Stream Deck button.");
  };

  const handlePickIcon = async (btn: StreamDeckButton) => {
    if (!pickFile) return;
    const p = await pickFile("Choose Custom Button PNG Icon", ["png", "jpg", "jpeg"]);
    if (!p) return;
    try {
      const res = await v2Client.importMediaFile(p);
      updateButton(btn.id, { icon_media: res.name });
      await v2Client.syncToKeyboard().catch(() => {});
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleTestPress = async (btn: StreamDeckButton) => {
    await v2Client.publishBus("modules/streamdeck/key", { button_id: btn.id, pressed: true }, false);
    window.setTimeout(() => {
      void v2Client.publishBus("modules/streamdeck/key", { button_id: btn.id, pressed: false }, false);
    }, 150);
    setStatusMsg(`Sent keyDown/keyUp for "${btn.title}".`);
  };

  const selected = buttons.find((b) => b.id === selectedBtnId) ?? buttons[0] ?? null;

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>Stream Deck</h1>
          <span className="kg-pill-subtle">.sdPlugin SDK Bridge · Bindable on Any QMK Layer</span>
        </div>
        <div className="kg-header-actions">
          <button
            type="button"
            className="kg-btn-primary"
            disabled={importing}
            onClick={() => void handleImportArchive()}
          >
            {importing ? "Importing…" : "+ Import .streamDeckPlugin"}
          </button>
          <button
            type="button"
            className="kg-btn-ghost"
            disabled={importing}
            onClick={() => void handleImportFolder()}
          >
            Import .sdPlugin Folder…
          </button>
        </div>
      </div>

      <p className="kg-page-desc">
        Import real Elgato Stream Deck plugins or create native live buttons, then bind each button to{" "}
        <strong>any physical key on any QMK layer</strong> (Base, Fn, Gaming, Layer 3+) in QMK Keymap. Switching
        layers automatically swaps your Stream Deck keycaps.
      </p>

      {/* Auto-detected Elgato plugins already on this Mac */}
      {elgatoDetected.length > 0 && (
        <div className="kg-calm-card">
          <div className="kg-row-title" style={{ marginBottom: 6 }}>
            Detected Elgato Stream Deck Plugins on This Mac
          </div>
          <div className="kg-chip-grid">
            {elgatoDetected.map((ep) => {
              const already = plugins.some((p) => p.id === ep.id);
              return (
                <button
                  key={ep.id}
                  type="button"
                  className="kg-kc-chip kg-kc-wide"
                  disabled={already || importing}
                  onClick={() => void handleImportFolder(ep.path)}
                >
                  <span>{ep.name}</span>
                  <small className="kg-muted">
                    {already ? "Imported" : `${ep.actions.length} actions · Click to import`}
                  </small>
                </button>
              );
            })}
          </div>
        </div>
      )}

      {/* Your Stream Deck Buttons (Bound to Any QMK Key/Layer!) */}
      <div className="kg-calm-card">
        <div className="kg-section-head">
          <div>
            <div className="kg-row-title">Your Stream Deck Buttons ({buttons.length})</div>
            <div className="kg-muted">
              Every button registers as a real bindable QMK action (`SD: ...`) — place it on any key &amp; layer.
            </div>
          </div>
          <button type="button" className="kg-btn-ghost" onClick={() => onOpenKeymap()}>
            Open QMK Keymap →
          </button>
        </div>

        {buttons.length === 0 ? (
          <div className="kg-muted">
            No Stream Deck buttons created yet — click any Built-in or Imported Plugin action below to add one!
          </div>
        ) : (
          <div style={{ display: "grid", gridTemplateColumns: "250px minmax(0, 1fr)", gap: 14, alignItems: "start" }}>
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {buttons.map((b) => {
                const bound = boundByBtnId[b.id];
                const isSel = selected?.id === b.id;
                return (
                  <button
                    key={b.id}
                    type="button"
                    className={`kg-nav-item ${isSel ? "is-active" : ""}`}
                    onClick={() => setSelectedBtnId(b.id)}
                  >
                    <span style={{ flex: 1 }}>
                      <div style={{ fontWeight: 600 }}>{b.title}</div>
                      <div className="kg-muted" style={{ fontSize: 11 }}>
                        {bound ? `Key ${bound.keyIndex} (L${bound.layer})` : "Unassigned"} · {b.plugin_id}
                      </div>
                    </span>
                  </button>
                );
              })}
            </div>

            {selected && (
              <div className="kg-settings-stack" style={{ paddingLeft: 12, borderLeft: "1px solid rgba(255,255,255,0.06)" }}>
                <div className="kg-calm-row" style={{ flexWrap: "wrap" }}>
                  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    <strong>{selected.title}</strong>
                    {boundByBtnId[selected.id] ? (
                      <button
                        type="button"
                        className="kg-status-badge is-ok"
                        style={{ border: "none", cursor: "pointer" }}
                        onClick={() => onOpenKeymap(boundByBtnId[selected.id].keyIndex)}
                      >
                        Bound · Key {boundByBtnId[selected.id].keyIndex} (L{boundByBtnId[selected.id].layer})
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="kg-status-badge is-warn"
                        style={{ border: "none", cursor: "pointer" }}
                        onClick={() => onOpenKeymap()}
                      >
                        Unassigned — Bind in Keymap →
                      </button>
                    )}
                  </div>
                  <div style={{ display: "flex", gap: 6 }}>
                    <button type="button" className="kg-btn-ghost" onClick={() => void handleTestPress(selected)}>
                      Test Press
                    </button>
                    <button type="button" className="kg-btn-ghost" onClick={() => deleteButton(selected.id)}>
                      Delete
                    </button>
                  </div>
                </div>

                <div className="kg-setting-row">
                  <div className="kg-setting-label">Keycap Title &amp; Accent</div>
                  <div className="kg-setting-ctrl">
                    <input
                      type="text"
                      value={selected.title}
                      onChange={(e) => updateButton(selected.id, { title: e.target.value })}
                      style={{ width: 160 }}
                    />
                    <input
                      type="color"
                      value={selected.color || "#5ce1ff"}
                      onChange={(e) => updateButton(selected.id, { color: e.target.value })}
                    />
                  </div>
                </div>

                <div className="kg-setting-row">
                  <div className="kg-setting-label">Custom PNG Keycap Icon</div>
                  <div className="kg-setting-ctrl">
                    <button type="button" className="kg-btn-ghost" onClick={() => void handlePickIcon(selected)}>
                      {selected.icon_media ? selected.icon_media : "Choose PNG…"}
                    </button>
                    {selected.icon_media && (
                      <button
                        type="button"
                        className="kg-btn-ghost"
                        onClick={() => updateButton(selected.id, { icon_media: null })}
                      >
                        Clear
                      </button>
                    )}
                  </div>
                </div>

                {/* Action-specific Settings */}
                {selected.action_uuid === "builtin.open_url" && (
                  <div className="kg-setting-row">
                    <div className="kg-setting-label">URL</div>
                    <div className="kg-setting-ctrl">
                      <input
                        type="text"
                        style={{ width: 280 }}
                        value={String(selected.settings.url ?? "")}
                        onChange={(e) =>
                          updateButton(selected.id, { settings: { ...selected.settings, url: e.target.value } })
                        }
                      />
                    </div>
                  </div>
                )}
                {selected.action_uuid === "builtin.launch_app" && (
                  <div className="kg-setting-row">
                    <div className="kg-setting-label">macOS Application Name</div>
                    <div className="kg-setting-ctrl">
                      <input
                        type="text"
                        style={{ width: 240 }}
                        value={String(selected.settings.app ?? "")}
                        onChange={(e) =>
                          updateButton(selected.id, { settings: { ...selected.settings, app: e.target.value } })
                        }
                      />
                    </div>
                  </div>
                )}
                {selected.action_uuid === "builtin.run_command" && (
                  <div className="kg-setting-row">
                    <div className="kg-setting-label">Shell Command</div>
                    <div className="kg-setting-ctrl">
                      <input
                        type="text"
                        style={{ width: 320 }}
                        value={String(selected.settings.command ?? "")}
                        onChange={(e) =>
                          updateButton(selected.id, { settings: { ...selected.settings, command: e.target.value } })
                        }
                      />
                    </div>
                  </div>
                )}
                {selected.action_uuid === "builtin.toggle_command" && (
                  <>
                    <div className="kg-setting-row">
                      <div className="kg-setting-label">ON Label &amp; Command</div>
                      <div className="kg-setting-ctrl">
                        <input
                          type="text"
                          style={{ width: 80 }}
                          value={String(selected.settings.on_title ?? "ON")}
                          onChange={(e) =>
                            updateButton(selected.id, {
                              settings: { ...selected.settings, on_title: e.target.value },
                            })
                          }
                        />
                        <input
                          type="text"
                          style={{ width: 220 }}
                          placeholder="Command when toggled ON"
                          value={String(selected.settings.on_command ?? "")}
                          onChange={(e) =>
                            updateButton(selected.id, {
                              settings: { ...selected.settings, on_command: e.target.value },
                            })
                          }
                        />
                      </div>
                    </div>
                    <div className="kg-setting-row">
                      <div className="kg-setting-label">OFF Label &amp; Command</div>
                      <div className="kg-setting-ctrl">
                        <input
                          type="text"
                          style={{ width: 80 }}
                          value={String(selected.settings.off_title ?? "OFF")}
                          onChange={(e) =>
                            updateButton(selected.id, {
                              settings: { ...selected.settings, off_title: e.target.value },
                            })
                          }
                        />
                        <input
                          type="text"
                          style={{ width: 220 }}
                          placeholder="Command when toggled OFF"
                          value={String(selected.settings.off_command ?? "")}
                          onChange={(e) =>
                            updateButton(selected.id, {
                              settings: { ...selected.settings, off_command: e.target.value },
                            })
                          }
                        />
                      </div>
                    </div>
                  </>
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Add Buttons from Imported .sdPlugin Packages */}
      {plugins.length > 0 && (
        <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div className="kg-row-title">Imported .sdPlugin Packages ({plugins.length})</div>
          {plugins.map((pl) => (
            <div key={pl.id} style={{ padding: 10, borderRadius: 10, background: "rgba(255,255,255,0.03)" }}>
              <div className="kg-calm-row" style={{ marginBottom: 8 }}>
                <div>
                  <strong>{pl.name}</strong>{" "}
                  <span className="kg-muted">
                    v{pl.version} · {pl.author} ({pl.actions.length} actions)
                  </span>
                </div>
                <button
                  type="button"
                  className="kg-btn-ghost"
                  onClick={async () => {
                    if (!window.confirm(`Remove imported .sdPlugin "${pl.name}"?`)) return;
                    await v2Client.deleteStreamDeckPlugin(pl.id);
                    await loadState();
                  }}
                >
                  Remove Plugin
                </button>
              </div>
              <div className="kg-chip-grid">
                {pl.actions.map((a) => {
                  const uuidSlug = a.uuid.replace(/[^A-Za-z0-9]/g, "_");
                  const defaultMedia = a.icon?.toLowerCase().endsWith(".png")
                    ? `sd_${pl.id}_${uuidSlug}.png`
                    : null;
                  return (
                    <button
                      key={a.uuid}
                      type="button"
                      className="kg-kc-chip kg-kc-wide"
                      onClick={() => handleAddButton(pl.id, a.uuid, a.name, "#5ce1ff", defaultMedia, {})}
                    >
                      <span>+ {a.name}</span>
                      <small className="kg-muted">{a.tooltip || a.uuid}</small>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Built-in Native Stream Deck Actions */}
      <div className="kg-calm-card">
        <div className="kg-row-title" style={{ marginBottom: 8 }}>
          Built-In Native Stream Deck Actions (Click to Create a Bindable Key)
        </div>
        <div className="kg-chip-grid">
          {BUILTIN_ACTIONS.map((a) => (
            <button
              key={a.uuid}
              type="button"
              className="kg-kc-chip kg-kc-wide"
              onClick={() => handleAddButton("builtin", a.uuid, a.defaultTitle, a.defaultColor, null, a.defaultSettings)}
            >
              <span>+ {a.name}</span>
              <small className="kg-muted">{a.desc}</small>
            </button>
          ))}
        </div>
      </div>

      {statusMsg && <div className="kg-muted">{statusMsg}</div>}
    </div>
  );
};
