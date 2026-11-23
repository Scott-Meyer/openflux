import type React from "react";
import { useEffect, useState } from "react";
import { type MediaLibraryFile, type V2ModuleRecord, v2Client } from "../api/v2Client";
import { KeyboardSurface, type SurfaceKeyGeom } from "./KeyboardSurface";

interface ModuleViewProps {
  mod: V2ModuleRecord;
  keys: SurfaceKeyGeom[];
  pickFile?: (title: string) => Promise<string | null>;
  onUpdated: () => Promise<void>;
  onOpenKeymap: () => void;
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
}) => {
  const [showPreview, setShowPreview] = useState(
    mod.manifest.category === "background" || mod.manifest.category === "effect",
  );
  const [previewTick, setPreviewTick] = useState(0);
  const [showLogs, setShowLogs] = useState(false);
  const [logs, setLogs] = useState<Array<{ timestamp_ms: number; level: string; message: string }>>([]);
  const [mediaFiles, setMediaFiles] = useState<MediaLibraryFile[]>([]);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  useEffect(() => {
    setShowPreview(mod.manifest.category === "background" || mod.manifest.category === "effect");
    setShowLogs(false);
    setStatusMsg(null);
  }, [mod.id, mod.manifest.category]);

  useEffect(() => {
    if (mod.manifest.settings.some((s) => s.kind === "file")) {
      void v2Client
        .listMediaLibrary()
        .then((r) => setMediaFiles(r.files))
        .catch(() => {});
    }
  }, [mod.id, mod.manifest.settings]);

  useEffect(() => {
    if (!showLogs) return;
    let alive = true;
    const poll = () => {
      void v2Client
        .getModuleLogs(mod.id)
        .then((r) => {
          if (alive) setLogs(r.logs);
        })
        .catch(() => {});
    };
    poll();
    const id = window.setInterval(poll, 2500);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [showLogs, mod.id]);

  const handleSettingChange = async (key: string, value: unknown) => {
    const nextCfg = { ...(mod.state.config ?? {}), [key]: value };
    try {
      await v2Client.updateModuleState(mod.id, { config: nextCfg });
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

  const handleTriggerAction = async (actionId: string) => {
    try {
      await v2Client.triggerAction(mod.id, actionId);
      setPreviewTick((t) => t + 1);
      setStatusMsg(`Triggered ${actionId}`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const hs = mod.host_status;

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>{mod.manifest.name}</h1>
          <span className="kg-pill-subtle">
            v{mod.manifest.version} · {mod.tier}
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
          <button
            type="button"
            className="kg-btn-ghost"
            onClick={() => setShowPreview((v) => !v)}
          >
            {showPreview ? "Hide Preview ▴" : "Preview ▾"}
          </button>
          <button type="button" className="kg-btn-ghost" onClick={() => void handleReload()}>
            Reload
          </button>
          <button type="button" className="kg-btn-ghost" onClick={() => void handleDisable()}>
            Disable
          </button>
        </div>
      </div>

      <p className="kg-page-desc">{mod.manifest.description}</p>

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
                  <div className="kg-setting-label">{field.label}</div>
                  {field.description && <div className="kg-muted">{field.description}</div>}
                </div>
                <div className="kg-setting-ctrl">
                  {field.kind === "boolean" ? (
                    <input
                      type="checkbox"
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

      {/* Advertised Keymap Actions */}
      {(mod.manifest.actions ?? []).length > 0 && (
        <div className="kg-calm-card">
          <div className="kg-section-head">
            <div>
              <div className="kg-row-title">Advertised Actions</div>
              <div className="kg-muted">Bind any of these actions to a physical key in QMK Keymap.</div>
            </div>
            <button type="button" className="kg-btn-ghost" onClick={onOpenKeymap}>
              Open Keymap →
            </button>
          </div>
          <div className="kg-actions-list">
            {(mod.manifest.actions ?? []).map((a) => (
              <div key={a.id} className="kg-action-row">
                <div>
                  <strong>{a.label}</strong>{" "}
                  <span className="kg-muted">({mod.id}.{a.id})</span>
                </div>
                <button
                  type="button"
                  className="kg-btn-ghost"
                  onClick={() => void handleTriggerAction(a.id)}
                >
                  Test
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Quiet Collapsible Logs & Dependencies */}
      <div className="kg-footer-tools">
        <button type="button" className="kg-btn-ghost" onClick={() => setShowLogs((v) => !v)}>
          {showLogs ? "Hide Logs ▴" : "Logs & Details ▾"}
        </button>
        {statusMsg && <span className="kg-muted">{statusMsg}</span>}
      </div>

      {showLogs && (
        <div className="kg-calm-card kg-logs-box">
          <div className="kg-muted" style={{ marginBottom: 6 }}>
            Path: {mod.path}
          </div>
          {logs.length === 0 ? (
            <div className="kg-muted">No recent logs.</div>
          ) : (
            <pre className="kg-logs-pre">
              {logs.slice(-30).map((l) => `[${l.level}] ${l.message}`).join("\n")}
            </pre>
          )}
        </div>
      )}
    </div>
  );
};
