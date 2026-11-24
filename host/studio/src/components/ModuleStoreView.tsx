import type React from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type CuratedListSubscription,
  type DisclosureReport,
  type V2ModuleRecord,
  v2Client,
} from "../api/v2Client";

interface ModuleStoreViewProps {
  modules: V2ModuleRecord[];
  onUpdated: () => Promise<void>;
  onOpenModule: (moduleId: string) => void;
}

interface CatalogEntry {
  id: string;
  name: string;
  version: string;
  category: string;
  description: string;
  source: string;
  author?: string;
  tags?: string[];
  runs_host_process?: boolean;
  sha256?: string;
  listName?: string;
}

const CATEGORY_TABS = [
  { id: "all", label: "All" },
  { id: "keys", label: "Keymaps & Legends" },
  { id: "widget", label: "Widgets" },
  { id: "background", label: "Backgrounds" },
  { id: "effect", label: "Effects" },
  { id: "host_service", label: "Integrations" },
] as const;

export const ModuleStoreView: React.FC<ModuleStoreViewProps> = ({
  modules,
  onUpdated,
  onOpenModule,
}) => {
  const [subTab, setSubTab] = useState<"installed" | "browse" | "lists">("installed");
  const [search, setSearch] = useState("");
  const [catFilter, setCatFilter] = useState<string>("all");
  const [sourceInput, setSourceInput] = useState("");
  const [lists, setLists] = useState<CuratedListSubscription[]>([]);
  const [catalogEntries, setCatalogEntries] = useState<CatalogEntry[]>([]);
  const [loadingCatalog, setLoadingCatalog] = useState(false);
  const [newListName, setNewListName] = useState("");
  const [newListSource, setNewListSource] = useState("");
  const [inspecting, setInspecting] = useState(false);
  const [pendingDisclosure, setPendingDisclosure] = useState<{
    source: string;
    inspectToken: string;
    disclosure: DisclosureReport;
  } | null>(null);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const loadCatalogs = useCallback(async () => {
    setLoadingCatalog(true);
    try {
      const res = await v2Client.listCuratedLists();
      const subs = res.lists ?? [];
      setLists(subs);
      const all: CatalogEntry[] = [];
      const seen = new Set<string>();
      for (const sub of subs) {
        try {
          const doc = await v2Client.browseCuratedList(sub.source);
          for (const e of doc.list?.entries ?? []) {
            if (!seen.has(e.id)) {
              seen.add(e.id);
              all.push({ ...(e as unknown as CatalogEntry), listName: sub.name });
            }
          }
        } catch {
          // Skip unreachable list without blocking others
        }
      }
      setCatalogEntries(all);
    } catch {
      // ignore
    } finally {
      setLoadingCatalog(false);
    }
  }, []);

  useEffect(() => {
    void loadCatalogs();
  }, [loadCatalogs]);

  const installedById = useMemo(() => {
    const m = new Map<string, V2ModuleRecord>();
    for (const mod of modules) m.set(mod.id, mod);
    return m;
  }, [modules]);

  const filteredInstalled = useMemo(() => {
    const q = search.trim().toLowerCase();
    return modules
      .filter((m) => {
        if (catFilter === "keys") {
          if (!m.id.startsWith("keys-") && !(m.manifest.tags ?? []).includes("keys")) return false;
        } else if (catFilter !== "all" && m.manifest.category !== catFilter) {
          return false;
        }
        if (!q) return true;
        return (
          m.manifest.name.toLowerCase().includes(q) ||
          m.manifest.description.toLowerCase().includes(q) ||
          m.id.toLowerCase().includes(q) ||
          (m.manifest.tags ?? []).some((t) => t.toLowerCase().includes(q))
        );
      })
      .sort(
        (a, b) =>
          Number(b.state.enabled) - Number(a.state.enabled) ||
          a.manifest.name.localeCompare(b.manifest.name),
      );
  }, [modules, search, catFilter]);

  const filteredCatalog = useMemo(() => {
    const q = search.trim().toLowerCase();
    return catalogEntries.filter((e) => {
      if (catFilter === "keys") {
        if (!e.id.startsWith("keys-") && !(e.tags ?? []).includes("keys")) return false;
      } else if (catFilter !== "all" && e.category !== catFilter) {
        return false;
      }
      if (!q) return true;
      return (
        e.name.toLowerCase().includes(q) ||
        e.description.toLowerCase().includes(q) ||
        e.id.toLowerCase().includes(q) ||
        (e.tags ?? []).some((t) => t.toLowerCase().includes(q))
      );
    });
  }, [catalogEntries, search, catFilter]);

  const handleToggle = async (mod: V2ModuleRecord) => {
    try {
      await v2Client.updateModuleState(mod.id, { enabled: !mod.state.enabled });
      await onUpdated();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleInspectSource = async (src: string) => {
    const clean = src.trim();
    if (!clean) return;
    setInspecting(true);
    setStatusMsg(null);
    try {
      const res = await v2Client.inspectModule(clean);
      setPendingDisclosure({
        source: clean,
        inspectToken: res.inspect_token,
        disclosure: res.preview.disclosure,
      });
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setInspecting(false);
    }
  };

  const handleApproveInstall = async () => {
    if (!pendingDisclosure) return;
    try {
      const res = await v2Client.installModule(
        pendingDisclosure.source,
        true,
        pendingDisclosure.inspectToken,
      );
      const installedId = res.preview.disclosure.module_id;
      setPendingDisclosure(null);
      setSourceInput("");
      await v2Client.updateModuleState(installedId, { enabled: true }).catch(() => {});
      await onUpdated();
      setStatusMsg(`Installed & enabled ${res.preview.disclosure.name} v${res.preview.disclosure.version}`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleAddList = async () => {
    if (!newListSource.trim()) return;
    const name = newListName.trim() || "Custom Module List";
    const id = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || `list-${Date.now()}`;
    try {
      await v2Client.addCuratedList({ id, name, source: newListSource.trim(), official: false });
      setNewListName("");
      setNewListSource("");
      await loadCatalogs();
      setStatusMsg(`Added module list "${name}".`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>Manage Modules</h1>
          <span className="kg-pill-subtle">
            {modules.filter((m) => m.state.enabled).length} active · {modules.length} installed ·{" "}
            {catalogEntries.length} in registries
          </span>
        </div>
        <div className="kg-segmented">
          <button
            type="button"
            className={`kg-seg-btn ${subTab === "installed" ? "is-active" : ""}`}
            onClick={() => setSubTab("installed")}
          >
            Installed ({modules.length})
          </button>
          <button
            type="button"
            className={`kg-seg-btn ${subTab === "browse" ? "is-active" : ""}`}
            onClick={() => setSubTab("browse")}
          >
            Browse Registries ({catalogEntries.length})
          </button>
          <button
            type="button"
            className={`kg-seg-btn ${subTab === "lists" ? "is-active" : ""}`}
            onClick={() => setSubTab("lists")}
          >
            Lists &amp; Direct Install
          </button>
        </div>
      </div>

      {/* TOCTOU-free Permission Disclosure Review Banner */}
      {pendingDisclosure && (
        <div className="kg-calm-card kg-disclosure-box" style={{ marginTop: 0 }}>
          <strong>
            Review Permissions: {pendingDisclosure.disclosure.name} v{pendingDisclosure.disclosure.version}
          </strong>
          <div className="kg-muted" style={{ margin: "6px 0" }}>
            Host execution:{" "}
            <strong>
              {pendingDisclosure.disclosure.runs_host_process
                ? `YES (${pendingDisclosure.disclosure.host_entrypoint}) — runs on your computer with your user permissions`
                : "None (sandboxed Lua only — no shell or filesystem write access)"}
            </strong>
          </div>
          <div className="kg-muted" style={{ fontSize: 11 }}>
            Verified staged snapshot ({pendingDisclosure.disclosure.files?.length ?? 0} files · installs exact inspected
            bytes, no second download).
          </div>
          <div style={{ display: "flex", gap: 8, marginTop: 10 }}>
            <button type="button" className="kg-btn-primary" onClick={() => void handleApproveInstall()}>
              Approve &amp; Install
            </button>
            <button type="button" className="kg-btn-ghost" onClick={() => setPendingDisclosure(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {statusMsg && <div className="kg-toast-banner" style={{ marginBottom: 0 }}>{statusMsg}</div>}

      {/* Shared Category Pills + Search for Installed & Browse tabs */}
      {(subTab === "installed" || subTab === "browse") && (
        <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
          <div className="kg-segmented">
            {CATEGORY_TABS.map((c) => (
              <button
                key={c.id}
                type="button"
                className={`kg-seg-btn ${catFilter === c.id ? "is-active" : ""}`}
                onClick={() => setCatFilter(c.id)}
              >
                {c.label}
              </button>
            ))}
          </div>
          <input
            type="text"
            style={{ flex: 1, minWidth: 200 }}
            placeholder="Search modules by name, description, or tag…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
      )}

      {/* TAB 1: Installed (Core + External Installed + Local) */}
      {subTab === "installed" && (
        <div className="kg-store-grid">
          {filteredInstalled.map((m) => (
            <div key={m.id} className="kg-calm-card kg-store-item">
              <div className="kg-calm-row">
                <div>
                  <strong>{m.manifest.name}</strong>{" "}
                  <span className="kg-muted" style={{ fontSize: 11 }}>
                    v{m.manifest.version} · {m.tier === "bundled" ? "Core" : m.tier} ·{" "}
                    {m.manifest.host_requirement === "none" ? "Sandboxed" : "Host Helper"}
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
      )}

      {/* TAB 2: Browse Subscribed Registries (KeyGlass Official + Community Taps) */}
      {subTab === "browse" && (
        <>
          {loadingCatalog && catalogEntries.length === 0 ? (
            <div className="kg-muted">Loading subscribed module registries…</div>
          ) : filteredCatalog.length === 0 ? (
            <div className="kg-calm-card kg-muted">
              No modules found in subscribed registries. Add a registry list under <strong>Lists &amp; Direct Install</strong>.
            </div>
          ) : (
            <div className="kg-store-grid">
              {filteredCatalog.map((e) => {
                const existing = installedById.get(e.id);
                return (
                  <div key={e.id} className="kg-calm-card kg-store-item">
                    <div className="kg-calm-row">
                      <div>
                        <strong>{e.name}</strong>{" "}
                        <span className="kg-muted" style={{ fontSize: 11 }}>
                          v{e.version} · {e.author || e.listName || "Registry"} ·{" "}
                          {e.runs_host_process ? "Host Helper" : "Sandboxed"}
                        </span>
                      </div>
                      <div style={{ display: "flex", gap: 6 }}>
                        {existing ? (
                          <>
                            <button type="button" className="kg-btn-ghost" onClick={() => onOpenModule(existing.id)}>
                              Open
                            </button>
                            <button
                              type="button"
                              className="kg-btn-ghost"
                              disabled={inspecting}
                              onClick={() => void handleInspectSource(e.source)}
                            >
                              Reinstall
                            </button>
                          </>
                        ) : (
                          <button
                            type="button"
                            className="kg-btn-primary"
                            disabled={inspecting}
                            onClick={() => void handleInspectSource(e.source)}
                          >
                            {inspecting ? "Inspecting…" : "Install"}
                          </button>
                        )}
                      </div>
                    </div>
                    <div className="kg-muted" style={{ fontSize: 12, marginTop: 6 }}>
                      {e.description}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </>
      )}

      {/* TAB 3: Subscribed Module Lists ("Taps") + Direct Install */}
      {subTab === "lists" && (
        <>
          <div className="kg-calm-card">
            <div className="kg-row-title" style={{ marginBottom: 6 }}>
              Direct Install (GitHub Repo, URL, Archive, or Local Folder)
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
                onClick={() => void handleInspectSource(sourceInput)}
              >
                {inspecting ? "Inspecting…" : "Inspect & Install"}
              </button>
            </div>
          </div>

          <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <div className="kg-row-title">Subscribed Module Registries (“Taps”)</div>
            <div className="kg-muted">
              Anyone can publish a static <code>catalog.json</code> on GitHub Pages — subscribe here to browse and
              update their modules right inside KeyGlass.
            </div>
            {lists.map((l) => (
              <div key={l.id} className="kg-action-row">
                <div>
                  <strong>{l.name}</strong> {l.official && <span className="kg-pill-accent">Official</span>}
                  <div className="kg-muted" style={{ fontSize: 11 }}>{l.source}</div>
                </div>
                {!l.official && (
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    onClick={async () => {
                      await v2Client.removeCuratedList(l.id);
                      await loadCatalogs();
                    }}
                  >
                    Remove
                  </button>
                )}
              </div>
            ))}
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <input
                type="text"
                placeholder="Registry Name"
                value={newListName}
                onChange={(e) => setNewListName(e.target.value)}
                style={{ width: 180 }}
              />
              <input
                type="text"
                style={{ flex: 1 }}
                placeholder="https://.../catalog.json or /path/to/catalog.json"
                value={newListSource}
                onChange={(e) => setNewListSource(e.target.value)}
              />
              <button
                type="button"
                className="kg-btn-primary"
                disabled={!newListSource.trim()}
                onClick={() => void handleAddList()}
              >
                + Add List
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
};
