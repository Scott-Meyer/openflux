import type React from "react";
import { useMemo, useState } from "react";
import { type DisclosureReport, type V2ModuleRecord, v2Client } from "../api/v2Client";

interface ModuleStoreViewProps {
  modules: V2ModuleRecord[];
  onUpdated: () => Promise<void>;
  onOpenModule: (moduleId: string) => void;
}

export const ModuleStoreView: React.FC<ModuleStoreViewProps> = ({
  modules,
  onUpdated,
  onOpenModule,
}) => {
  const [search, setSearch] = useState("");
  const [sourceInput, setSourceInput] = useState("");
  const [inspecting, setInspecting] = useState(false);
  const [pendingDisclosure, setPendingDisclosure] = useState<{
    source: string;
    disclosure: DisclosureReport;
  } | null>(null);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return modules
      .filter((m) => {
        if (!q) return true;
        return (
          m.manifest.name.toLowerCase().includes(q) ||
          m.manifest.description.toLowerCase().includes(q) ||
          m.id.toLowerCase().includes(q) ||
          (m.manifest.tags ?? []).some((t) => t.toLowerCase().includes(q))
        );
      })
      .sort((a, b) => Number(a.state.enabled) - Number(b.state.enabled) || a.manifest.name.localeCompare(b.manifest.name));
  }, [modules, search]);

  const handleToggle = async (mod: V2ModuleRecord) => {
    try {
      await v2Client.updateModuleState(mod.id, { enabled: !mod.state.enabled });
      await onUpdated();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleInspectSource = async () => {
    if (!sourceInput.trim()) return;
    setInspecting(true);
    setStatusMsg(null);
    try {
      const res = await v2Client.inspectModule(sourceInput.trim());
      setPendingDisclosure({ source: sourceInput.trim(), disclosure: res.preview.disclosure });
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setInspecting(false);
    }
  };

  const handleApproveInstall = async () => {
    if (!pendingDisclosure) return;
    try {
      const res = await v2Client.installModule(pendingDisclosure.source, true);
      setPendingDisclosure(null);
      setSourceInput("");
      await onUpdated();
      setStatusMsg(`Installed ${res.preview.disclosure.name} v${res.preview.disclosure.version}`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>Add Module</h1>
          <span className="kg-pill-subtle">{modules.length} available</span>
        </div>
      </div>

      {/* Install External Module (GitHub / URL / Local Folder) */}
      <div className="kg-calm-card">
        <div className="kg-row-title" style={{ marginBottom: 6 }}>
          Install from GitHub, URL, Archive, or Local Folder
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            type="text"
            style={{ flex: 1 }}
            placeholder="github:owner/repo, https://..., or /path/to/module"
            value={sourceInput}
            onChange={(e) => setSourceInput(e.target.value)}
          />
          <button
            type="button"
            className="kg-btn-primary"
            disabled={inspecting || !sourceInput.trim()}
            onClick={() => void handleInspectSource()}
          >
            {inspecting ? "Inspecting…" : "Inspect & Install"}
          </button>
        </div>

        {pendingDisclosure && (
          <div className="kg-disclosure-box">
            <strong>
              Review Permissions: {pendingDisclosure.disclosure.name} v{pendingDisclosure.disclosure.version}
            </strong>
            <div className="kg-muted" style={{ margin: "6px 0" }}>
              Host execution:{" "}
              <strong>
                {pendingDisclosure.disclosure.runs_host_process
                  ? `YES (${pendingDisclosure.disclosure.host_entrypoint}) — runs on your computer with your user permissions`
                  : "None (sandboxed Lua only)"}
              </strong>
            </div>
            <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
              <button type="button" className="kg-btn-primary" onClick={() => void handleApproveInstall()}>
                Approve &amp; Install
              </button>
              <button type="button" className="kg-btn-ghost" onClick={() => setPendingDisclosure(null)}>
                Cancel
              </button>
            </div>
          </div>
        )}
        {statusMsg && <div className="kg-muted" style={{ marginTop: 8 }}>{statusMsg}</div>}
      </div>

      {/* Search & Browse Installed / Bundled Modules */}
      <div style={{ display: "flex", gap: 8 }}>
        <input
          type="text"
          style={{ flex: 1 }}
          placeholder="Search modules by name, description, or tag…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      <div className="kg-store-grid">
        {filtered.map((m) => (
          <div key={m.id} className="kg-calm-card kg-store-item">
            <div className="kg-calm-row">
              <div>
                <strong>{m.manifest.name}</strong>{" "}
                <span className="kg-muted" style={{ fontSize: 11 }}>
                  v{m.manifest.version} · {m.manifest.host_requirement === "none" ? "Standalone" : "Host Helper"}
                </span>
              </div>
              <div style={{ display: "flex", gap: 6 }}>
                {m.state.enabled && (
                  <button type="button" className="kg-btn-ghost" onClick={() => onOpenModule(m.id)}>
                    Open
                  </button>
                )}
                <button
                  type="button"
                  className={m.state.enabled ? "kg-btn-ghost" : "kg-btn-primary"}
                  onClick={() => void handleToggle(m)}
                >
                  {m.state.enabled ? "Enabled" : "Enable"}
                </button>
              </div>
            </div>
            <div className="kg-muted" style={{ fontSize: 12, marginTop: 6 }}>
              {m.manifest.description}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};
