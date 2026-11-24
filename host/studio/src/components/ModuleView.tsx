import type React from "react";
import { useEffect, useState } from "react";
import {
  type MediaLibraryFile,
  type ModuleUpdateInfo,
  type V2ModuleRecord,
  v2Client,
} from "../api/v2Client";
import { BindingImportPanel } from "./BindingImportPanel";
import { KeyboardSurface, type SurfaceKeyGeom } from "./KeyboardSurface";

interface ModuleViewProps {
  mod: V2ModuleRecord;
  keys: SurfaceKeyGeom[];
  pickFile?: (title: string) => Promise<string | null>;
  onUpdated: () => Promise<void>;
  onOpenKeymap: (focusKey?: number) => void;
  onUninstalled?: () => void;
}

const KEY_INDEX_OPTIONS: Array<{ value: number; label: string }> = [
  { value: 0, label: "Key 0 (Esc)" },
  { value: 12, label: "Key 12 (F12)" },
  { value: 13, label: "Key 13 (PrtSc)" },
  { value: 14, label: "Key 14 (Ins)" },
  { value: 15, label: "Key 15 (Del)" },
  { value: 30, label: "Key 30 (Home)" },
  { value: 45, label: "Key 45 (End)" },
  { value: 59, label: "Key 59 (PgUp)" },
  { value: 73, label: "Key 73 (PgDn)" },
  { value: 80, label: "Key 80 (Bottom-Right 1)" },
  { value: 81, label: "Key 81 (Bottom-Right 2)" },
];

export const ModuleView: React.FC<ModuleViewProps> = ({
  mod,
  keys,
  pickFile,
  onUpdated,
  onOpenKeymap,
  onUninstalled,
}) => {
  const [showPreview, setShowPreview] = useState(
    mod.manifest.category === "background" || mod.manifest.category === "effect",
  );
  const [previewTick, setPreviewTick] = useState(0);
  const [showLogs, setShowLogs] = useState(false);
  const [logs, setLogs] = useState<Array<{ timestamp_ms: number; level: string; message: string }>>([]);
  const [readme, setReadme] = useState<string | null>(null);
  const [mediaFiles, setMediaFiles] = useState<MediaLibraryFile[]>([]);
  const [boundByActionId, setBoundByActionId] = useState<Record<string, { keyIndex: number; layer: number }>>({});
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const [busyOp, setBusyOp] = useState<string | null>(null);
  const [availUpdate, setAvailUpdate] = useState<ModuleUpdateInfo | null>(null);

  useEffect(() => {
    setShowPreview(mod.manifest.category === "background" || mod.manifest.category === "effect");
    setShowLogs(false);
    setStatusMsg(null);
    setReadme(null);
    setAvailUpdate(null);
    if (mod.tier === "installed") {
      let alive = true;
      void v2Client
        .checkModuleUpdates()
        .then((res) => {
          if (!alive) return;
          const match = (res.updates ?? []).find(
            (u) => u.id === mod.id && (u.has_update || u.deprecated || u.moved_to || u.replaced_by),
          );
          setAvailUpdate(match ?? null);
        })
        .catch(() => {});
      return () => {
        alive = false;
      };
    }
  }, [mod.id, mod.tier, mod.manifest.version, mod.manifest.category]);

  useEffect(() => {
    if (mod.manifest.settings.some((s) => s.kind === "file")) {
      void v2Client
        .listMediaLibrary()
        .then((r) => setMediaFiles(r.files))
        .catch(() => {});
    }
  }, [mod.id, mod.manifest.settings]);

  // Check which of this module's advertised `[[actions]]` are currently bound to physical QMK keys!
  useEffect(() => {
    if ((mod.manifest.actions ?? []).length === 0) return;
    let alive = true;
    void Promise.all([
      v2Client.getBusRetained("system/actions"),
      v2Client.getBusRetained("keyboard/resolved_keys"),
    ])
      .then(([saRes, rkRes]) => {
        if (!alive) return;
        const slots = (saRes.messages[0]?.payload as { slots?: Record<string, { action_id: string; keycode: number }> })
          ?.slots;
        const rkKeys = (rkRes.messages[0]?.payload as { keys?: Array<{ keycode: number; source_layer: number }> })
          ?.keys;
        if (!slots || !rkKeys) return;
        const kcToAction = new Map<number, string>();
        for (const entry of Object.values(slots)) {
          if (typeof entry.keycode === "number" && entry.action_id) {
            kcToAction.set(entry.keycode, entry.action_id);
          }
        }
        const map: Record<string, { keyIndex: number; layer: number }> = {};
        rkKeys.forEach((rk, idx) => {
          const aid = kcToAction.get(rk.keycode);
          if (aid && !map[aid]) {
            map[aid] = { keyIndex: idx, layer: rk.source_layer ?? 0 };
          }
        });
        setBoundByActionId(map);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [mod.id, mod.manifest.actions, previewTick]);

  useEffect(() => {
    if (!showLogs) return;
    let alive = true;
    const poll = () => {
      void v2Client
        .getModuleDetail(mod.id)
        .then((r) => {
          if (!alive) return;
          setLogs(r.logs ?? []);
          setReadme(r.readme ?? null);
        })
        .catch(() => {});
    };
    poll();
    const id = window.setInterval(poll, 3000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [showLogs, mod.id]);

  const handleSettingChange = async (key: string, value: unknown) => {
    try {
      await v2Client.updateModuleState(mod.id, { config: { [key]: value } });
      await onUpdated();
      setPreviewTick((t) => t + 1);
      setStatusMsg("Saved");
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDisable = async () => {
    await v2Client.updateModuleState(mod.id, { enabled: false });
    await onUpdated();
  };

  const handleReload = async () => {
    await v2Client.reloadModule(mod.id);
    await onUpdated();
    setPreviewTick((t) => t + 1);
    setStatusMsg("Reloaded");
  };

  const handleUpdate = async () => {
    setBusyOp("update");
    setStatusMsg(null);
    try {
      const check = await v2Client.updateModule(mod.id, false);
      if (check.requires_acceptance) {
        const ok = window.confirm(
          `Update "${mod.manifest.name}" to v${check.preview.disclosure.version}? Review any permission changes before continuing.`,
        );
        if (!ok) {
          setBusyOp(null);
          return;
        }
        await v2Client.updateModule(mod.id, true);
      }
      await onUpdated();
      setAvailUpdate(null);
      setStatusMsg(`Updated ${mod.manifest.name}`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyOp(null);
    }
  };

  const handleForkToLocal = async () => {
    const suggestedId = `${mod.id}-local`;
    const newId = window.prompt(
      `Fork "${mod.manifest.name}" to an editable local module.\n\nNew local module ID (lowercase letters, numbers, hyphens):`,
      suggestedId,
    );
    if (!newId || !newId.trim()) return;
    setBusyOp("fork");
    try {
      await v2Client.forkModule({
        source_id: mod.id,
        new_id: newId.trim(),
        name: `${mod.manifest.name} (Local Fork)`,
      });
      await onUpdated();
      setStatusMsg(`Forked to ~/.local/share/openflux/v2/modules/local/${newId.trim()} — edit script.lua and click Reload!`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyOp(null);
    }
  };

  const handleUninstall = async () => {
    if (!window.confirm(`Uninstall "${mod.manifest.name}" (${mod.tier})? This deletes its folder from Keyglass.`)) {
      return;
    }
    setBusyOp("uninstall");
    try {
      await v2Client.uninstallModule(mod.id);
      await onUpdated();
      onUninstalled?.();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyOp(null);
    }
  };

  const handleTriggerAction = async (actionId: string) => {
    try {
      await v2Client.triggerAction(mod.id, actionId);
      setPreviewTick((t) => t + 1);
      setStatusMsg(`Triggered ${actionId}`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCopyDebugInfo = async () => {
    const subs = mod.manifest.channels?.subscribes ?? [];
    const pubs = mod.manifest.channels?.publishes ?? [];
    const recentLogs = logs
      .slice(-25)
      .map((l) => `[${l.level}] ${l.message}`)
      .join("\n");
    const report = [
      `### Keyglass Module Diagnostics`,
      `- **Module:** ${mod.manifest.name} (\`${mod.id}\`) v${mod.manifest.version} (${mod.tier})`,
      `- **Author:** ${mod.manifest.author || "Unknown"}${mod.manifest.license ? ` · License: ${mod.manifest.license}` : ""}`,
      `- **Healthy:** ${mod.healthy}${mod.last_error ? ` — \`${mod.last_error}\`` : ""}`,
      `- **Host Status:** ${JSON.stringify(mod.host_status ?? { state: "not_required" })}`,
      `- **Subscribes:** ${subs.join(", ") || "none"}`,
      `- **Publishes:** ${pubs.join(", ") || "none"}`,
      `- **Path:** \`${mod.path}\``,
      recentLogs ? `\n\`\`\`\n${recentLogs}\n\`\`\`` : "\n_(no recent logs)_",
    ].join("\n");
    try {
      await navigator.clipboard.writeText(report);
      setStatusMsg("Copied diagnostic bundle to clipboard.");
    } catch {
      setStatusMsg("Could not access clipboard.");
    }
  };

  const hs = mod.host_status;
  const canUninstall = mod.tier !== "bundled";
  const supportUrl = mod.manifest.support_url || mod.manifest.issues_url;

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>{mod.manifest.name}</h1>
          <span className="kg-pill-subtle">
            v{mod.manifest.version} · {mod.tier === "bundled" ? "Core" : mod.tier}
            {mod.manifest.forked_from ? ` (fork of ${mod.manifest.forked_from})` : ""}
          </span>
          {hs && hs.state !== "not_required" && (
            <span className={`kg-status-badge ${hs.state === "running" ? "is-ok" : "is-err"}`}>
              {hs.state === "running"
                ? `Host PID ${hs.pid}`
                : hs.state === "exited"
                  ? `Host Exited (${hs.code ?? "sig"})`
                  : "Host Error"}
            </span>
          )}
        </div>
        <div className="kg-header-actions">
          <button type="button" className="kg-btn-ghost" onClick={() => setShowPreview((v) => !v)}>
            {showPreview ? "Hide Preview ▴" : "Preview ▾"}
          </button>
          <button type="button" className="kg-btn-ghost" onClick={() => void handleReload()}>
            Reload
          </button>
          {mod.tier !== "local" && (
            <button
              type="button"
              className="kg-btn-ghost"
              disabled={busyOp !== null}
              onClick={() => void handleForkToLocal()}
              title="Copy into your local modules folder so you can customize its code"
            >
              {busyOp === "fork" ? "Forking…" : "Fork to Local"}
            </button>
          )}
          <button type="button" className="kg-btn-ghost" onClick={() => void handleDisable()}>
            Disable
          </button>
          {canUninstall && (
            <button
              type="button"
              className="kg-btn-ghost kg-btn-danger-ghost"
              disabled={busyOp === "uninstall"}
              onClick={() => void handleUninstall()}
            >
              Uninstall
            </button>
          )}
        </div>
      </div>

      {/* Deprecation / Move / Rename Banner */}
      {(mod.manifest.deprecated ||
        mod.manifest.moved_to ||
        availUpdate?.deprecated ||
        availUpdate?.moved_to) && (
        <div className="kg-calm-card" style={{ marginTop: 0, borderColor: "rgba(251, 191, 36, 0.45)" }}>
          <div className="kg-calm-row">
            <div>
              <strong style={{ color: "#fbbf24" }}>
                {availUpdate?.moved_to || mod.manifest.moved_to
                  ? "This Module Has Moved / Been Replaced"
                  : "Deprecated Module"}
              </strong>
              <div className="kg-muted" style={{ fontSize: 12, marginTop: 4 }}>
                {availUpdate?.deprecation_message ||
                  mod.manifest.deprecation_message ||
                  "The author marked this module as deprecated."}
              </div>
              {(availUpdate?.moved_to || mod.manifest.moved_to) && (
                <div className="kg-muted" style={{ fontSize: 11, marginTop: 4 }}>
                  New location: <code>{availUpdate?.moved_to || mod.manifest.moved_to}</code>
                  {(availUpdate?.replaced_by || mod.manifest.replaced_by) && (
                    <>
                      {" "}
                      (new ID: <code>{availUpdate?.replaced_by || mod.manifest.replaced_by}</code>)
                    </>
                  )}
                </div>
              )}
            </div>
            {(availUpdate?.moved_to || mod.manifest.moved_to) && (
              <button
                type="button"
                className="kg-btn-primary"
                disabled={busyOp !== null}
                onClick={async () => {
                  const dest = availUpdate?.moved_to || mod.manifest.moved_to;
                  if (!dest) return;
                  if (
                    !window.confirm(
                      `Migrate "${mod.manifest.name}" to:\n${dest}\n\nYour saved settings and slot placement will carry over, and the old module will be removed.`,
                    )
                  ) {
                    return;
                  }
                  setBusyOp("migrate");
                  try {
                    const insp = await v2Client.inspectModule(dest);
                    const res = await v2Client.migrateModule(mod.id, dest, true, insp.inspect_token);
                    await onUpdated();
                    setStatusMsg(`Migrated ${res.old_id} → ${res.new_id}!`);
                  } catch (err) {
                    setStatusMsg(err instanceof Error ? err.message : String(err));
                  } finally {
                    setBusyOp(null);
                  }
                }}
              >
                {busyOp === "migrate" ? "Migrating…" : "Migrate Now"}
              </button>
            )}
          </div>
        </div>
      )}

      {/* Update Banner — ONLY shown when an actual upstream update exists! */}
      {availUpdate && availUpdate.has_update && !availUpdate.moved_to && (
        <div className="kg-calm-card kg-disclosure-box" style={{ marginTop: 0 }}>
          <div className="kg-calm-row">
            <div>
              <strong>
                Update Available: v{availUpdate.current_version} → v{availUpdate.available_version}
              </strong>
              <span className="kg-muted" style={{ fontSize: 11, marginLeft: 8 }}>
                via {availUpdate.list_name || "Direct Source"}
              </span>
              {availUpdate.changelog && (
                <div className="kg-muted" style={{ fontSize: 12, marginTop: 4 }}>
                  {availUpdate.changelog}
                </div>
              )}
              {availUpdate.host_changed && (
                <div style={{ fontSize: 11, color: "#fbbf24", marginTop: 4 }}>
                  Note: This update changes host-helper permissions; you will review changes before applying.
                </div>
              )}
            </div>
            <button
              type="button"
              className="kg-btn-primary"
              disabled={busyOp === "update"}
              onClick={() => void handleUpdate()}
            >
              {busyOp === "update" ? "Updating…" : `Update to v${availUpdate.available_version}`}
            </button>
          </div>
        </div>
      )}

      <p className="kg-page-desc">{mod.manifest.description}</p>

      {/* Rich Module Metadata & Community Links */}
      <div
        className="kg-muted"
        style={{ display: "flex", flexWrap: "wrap", gap: "4px 12px", fontSize: 11, alignItems: "center", marginTop: -8 }}
      >
        <span>
          By <strong>{mod.manifest.author || "Keyglass"}</strong>
        </span>
        {mod.manifest.license && <span>License: {mod.manifest.license}</span>}
        {mod.origin?.source && (
          <span>
            Source: <code>{mod.origin.source}</code>
          </span>
        )}
        {mod.tier === "local" && (
          <span>
            Local Path: <code>{mod.path}</code> (auto-reloads on edit)
          </span>
        )}
        {mod.manifest.homepage && (
          <a href={mod.manifest.homepage} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
            Homepage ↗
          </a>
        )}
        {mod.manifest.repository && (
          <a href={mod.manifest.repository} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
            Source ↗
          </a>
        )}
        {supportUrl && (
          <a href={supportUrl} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
            Support / Issues ↗
          </a>
        )}
        {mod.manifest.discord_url && (
          <a href={mod.manifest.discord_url} target="_blank" rel="noreferrer" style={{ color: "#a5b4fc" }}>
            Discord ↗
          </a>
        )}
        {mod.manifest.changelog_url && (
          <a href={mod.manifest.changelog_url} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
            Changelog ↗
          </a>
        )}
      </div>
      {mod.manifest.changelog && !availUpdate && (
        <div className="kg-muted" style={{ fontSize: 11, marginTop: -4 }}>
          Release notes (v{mod.manifest.version}): {mod.manifest.changelog}
        </div>
      )}

      {!mod.healthy && mod.last_error && (
        <div className="kg-error-banner">
          <strong>Module Error:</strong> {mod.last_error}
        </div>
      )}

      {showPreview && (
        <div className="kg-surface-wrap kg-surface-compact">
          <KeyboardSurface keys={keys} selectedKey={null} showLivePreview previewTick={previewTick} />
        </div>
      )}

      {mod.manifest.ui?.builtin_view === "binding-import" && (
        <BindingImportPanel key={mod.id} moduleId={mod.id} moduleSnapshot={mod} />
      )}

      {/* Clean Settings Card */}
      <div className="kg-calm-card kg-settings-stack">
        {mod.manifest.settings.length === 0 ? (
          <div className="kg-muted">This module has no configurable settings.</div>
        ) : (
          mod.manifest.settings.map((field) => {
            const cur = mod.state.config?.[field.key] ?? field.default ?? "";
            return (
              <div key={field.key} className="kg-setting-row">
                <div className="kg-setting-meta">
                  {field.kind === "boolean" ? (
                    <label className="kg-setting-label" htmlFor={`module-setting-${field.key}`}>
                      {field.label}
                    </label>
                  ) : (
                    <div className="kg-setting-label">{field.label}</div>
                  )}
                  {field.description && <div className="kg-muted">{field.description}</div>}
                </div>
                <div className="kg-setting-ctrl">
                  {field.kind === "boolean" ? (
                    <input
                      type="checkbox"
                      id={`module-setting-${field.key}`}
                      checked={Boolean(cur)}
                      onChange={(e) => void handleSettingChange(field.key, e.target.checked)}
                    />
                  ) : field.kind === "select" ? (
                    <select
                      value={String(cur)}
                      onChange={(e) => void handleSettingChange(field.key, e.target.value)}
                    >
                      {(field.options ?? []).map((opt) => (
                        <option key={opt} value={opt}>
                          {opt}
                        </option>
                      ))}
                    </select>
                  ) : field.kind === "number" ? (
                    <div className="kg-range-wrap">
                      <input
                        type="range"
                        min={field.min ?? 0}
                        max={field.max ?? 100}
                        step={(field.max ?? 100) - (field.min ?? 0) <= 5 ? 0.05 : 1}
                        value={Number(cur) || 0}
                        onChange={(e) => void handleSettingChange(field.key, Number(e.target.value))}
                      />
                      <span className="kg-range-val">{String(cur)}</span>
                    </div>
                  ) : field.kind === "color" ? (
                    <input
                      type="color"
                      value={String(cur || "#4de1ff")}
                      onChange={(e) => void handleSettingChange(field.key, e.target.value)}
                    />
                  ) : field.kind === "key" ? (
                    <select
                      value={Number(cur) || 0}
                      onChange={(e) => void handleSettingChange(field.key, Number(e.target.value))}
                    >
                      {KEY_INDEX_OPTIONS.map((k) => (
                        <option key={k.value} value={k.value}>
                          {k.label}
                        </option>
                      ))}
                    </select>
                  ) : field.kind === "file" ? (
                    <div className="kg-file-ctrl">
                      {mediaFiles.length > 0 && (
                        <select
                          value={String(cur)}
                          onChange={(e) => void handleSettingChange(field.key, e.target.value)}
                        >
                          <option value="">Choose library media…</option>
                          {mediaFiles.map((f) => (
                            <option key={f.path} value={f.path}>
                              {f.name}
                            </option>
                          ))}
                        </select>
                      )}
                      {pickFile && (
                        <button
                          type="button"
                          className="kg-btn-ghost"
                          onClick={async () => {
                            const p = await pickFile(`Select ${field.label}`);
                            if (p) void handleSettingChange(field.key, p);
                          }}
                        >
                          Browse…
                        </button>
                      )}
                    </div>
                  ) : (
                    <input
                      type={field.secret ? "password" : "text"}
                      value={String(cur)}
                      onChange={(e) => void handleSettingChange(field.key, e.target.value)}
                    />
                  )}
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Advertised Keymap Actions with LIVE Bound / Unassigned Status */}
      {(mod.manifest.actions ?? []).length > 0 && (
        <div className="kg-calm-card">
          <div className="kg-section-head">
            <div>
              <div className="kg-row-title">Advertised Keymap Actions</div>
              <div className="kg-muted">Bind any of these actions to a physical key in QMK Keymap.</div>
            </div>
            <button type="button" className="kg-btn-ghost" onClick={() => onOpenKeymap()}>
              Open Keymap →
            </button>
          </div>
          <div className="kg-actions-list">
            {(mod.manifest.actions ?? []).map((a) => {
              const fullId = `${mod.id}.${a.id}`;
              const bound = boundByActionId[fullId] ?? boundByActionId[`${mod.id}:${a.id}`];
              return (
                <div key={a.id} className="kg-action-row">
                  <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                    <strong>{a.label}</strong>
                    {bound ? (
                      <button
                        type="button"
                        className="kg-status-badge is-ok"
                        style={{ border: "none", cursor: "pointer" }}
                        onClick={() => onOpenKeymap(bound.keyIndex)}
                        title="Click to edit this key in QMK Keymap"
                      >
                        Bound · Key {bound.keyIndex} (L{bound.layer})
                      </button>
                    ) : (
                      <span className="kg-status-badge is-warn">Unassigned</span>
                    )}
                  </div>
                  <div style={{ display: "flex", gap: 6 }}>
                    <button
                      type="button"
                      className="kg-btn-ghost"
                      onClick={() => void handleTriggerAction(a.id)}
                    >
                      Test
                    </button>
                    {!bound && (
                      <button type="button" className="kg-btn-ghost" onClick={() => onOpenKeymap()}>
                        Bind →
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Quiet Collapsible Troubleshooting, Wiring & Support */}
      <div className="kg-footer-tools">
        <button type="button" className="kg-btn-ghost" onClick={() => setShowLogs((v) => !v)}>
          {showLogs ? "Hide Logs, Info & Support ▴" : "Logs, Info & Support ▾"}
        </button>
        <button type="button" className="kg-btn-ghost" onClick={() => void handleCopyDebugInfo()}>
          Copy Debug Info
        </button>
        {statusMsg && <span className="kg-muted">{statusMsg}</span>}
      </div>

      {showLogs && (
        <div className="kg-calm-card kg-logs-box" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))", gap: 10 }}>
            <div>
              <div className="kg-group-label">Module Info</div>
              <div className="kg-muted" style={{ marginTop: 4 }}>
                Author: <strong>{mod.manifest.author || "Community"}</strong>
                {mod.manifest.license ? ` · ${mod.manifest.license}` : ""}
              </div>
              <div className="kg-muted" style={{ marginTop: 2, wordBreak: "break-all" }}>
                Folder: <code>{mod.path}</code>
              </div>
              {(mod.manifest.homepage || mod.manifest.repository || mod.manifest.issues_url) && (
                <div style={{ display: "flex", gap: 10, marginTop: 6, fontSize: 12 }}>
                  {mod.manifest.homepage && (
                    <a href={mod.manifest.homepage} target="_blank" rel="noreferrer" style={{ color: "#93c5fd" }}>
                      Homepage ↗
                    </a>
                  )}
                  {mod.manifest.repository && (
                    <a href={mod.manifest.repository} target="_blank" rel="noreferrer" style={{ color: "#93c5fd" }}>
                      Source ↗
                    </a>
                  )}
                  {mod.manifest.issues_url && (
                    <a href={mod.manifest.issues_url} target="_blank" rel="noreferrer" style={{ color: "#93c5fd" }}>
                      Report Issue ↗
                    </a>
                  )}
                </div>
              )}
            </div>
            <div>
              <div className="kg-group-label">Bus Wiring (Who Talks to Whom)</div>
              <div className="kg-muted" style={{ marginTop: 4 }}>
                <strong>Subscribes:</strong>{" "}
                {(mod.manifest.channels?.subscribes ?? []).join(", ") || "none"}
              </div>
              <div className="kg-muted" style={{ marginTop: 2 }}>
                <strong>Publishes:</strong>{" "}
                {(mod.manifest.channels?.publishes ?? []).join(", ") || "none"}
              </div>
              {(mod.manifest.provides ?? []).length > 0 && (
                <div className="kg-muted" style={{ marginTop: 2 }}>
                  <strong>Provides role:</strong> {(mod.manifest.provides ?? []).join(", ")}
                </div>
              )}
            </div>
          </div>

          {readme && (
            <div>
              <div className="kg-group-label">README</div>
              <pre className="kg-logs-pre" style={{ marginTop: 4, maxHeight: 160 }}>
                {readme}
              </pre>
            </div>
          )}

          <div>
            <div className="kg-group-label">Recent Logs</div>
            {logs.length === 0 ? (
              <div className="kg-muted" style={{ marginTop: 4 }}>No recent logs.</div>
            ) : (
              <pre className="kg-logs-pre" style={{ marginTop: 4 }}>
                {logs.slice(-30).map((l) => `[${l.level}] ${l.message}`).join("\n")}
              </pre>
            )}
          </div>
        </div>
      )}
    </div>
  );
};
