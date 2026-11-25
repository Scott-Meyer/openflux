import type React from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type CuratedListSubscription,
  type DisclosureReport,
  type ModuleCategory,
  type ModuleUpdateInfo,
  type RemovedModuleRecord,
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
  author?: string | null;
  license?: string | null;
  homepage?: string | null;
  repository?: string | null;
  support_url?: string | null;
  discord_url?: string | null;
  changelog?: string | null;
  changelog_url?: string | null;
  maturity?: "stable" | "beta" | "alpha" | "deprecated" | "abandoned";
  deprecated?: boolean;
  deprecation_message?: string | null;
  moved_to?: string | null;
  replaced_by?: string | null;
  tags?: string[];
  runs_host_process?: boolean;
  sha256?: string | null;
  listId?: string;
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

const MaturityBadge: React.FC<{ maturity?: string; deprecated?: boolean }> = ({ maturity, deprecated }) => {
  const m = (maturity || (deprecated ? "deprecated" : "stable")).toLowerCase();
  if (m === "stable" && !deprecated) return null;
  const styles: Record<string, { label: string; bg: string; color: string; border: string }> = {
    alpha: {
      label: "Alpha",
      bg: "rgba(245, 158, 11, 0.16)",
      color: "#fbbf24",
      border: "rgba(245, 158, 11, 0.4)",
    },
    beta: {
      label: "Beta",
      bg: "rgba(99, 102, 241, 0.18)",
      color: "#a5b4fc",
      border: "rgba(129, 140, 248, 0.45)",
    },
    deprecated: {
      label: "Deprecated",
      bg: "rgba(239, 68, 68, 0.14)",
      color: "#fca5a5",
      border: "rgba(248, 113, 113, 0.4)",
    },
    abandoned: {
      label: "Abandoned",
      bg: "rgba(239, 68, 68, 0.16)",
      color: "#f87171",
      border: "rgba(248, 113, 113, 0.45)",
    },
  };
  const st = styles[m] ?? styles.deprecated;
  return (
    <span
      style={{
        display: "inline-block",
        marginLeft: 6,
        padding: "1px 6px",
        borderRadius: 5,
        fontSize: 10,
        fontWeight: 600,
        letterSpacing: "0.02em",
        background: st.bg,
        color: st.color,
        border: `1px solid ${st.border}`,
        verticalAlign: "middle",
      }}
    >
      {st.label}
    </span>
  );
};

export const ModuleStoreView: React.FC<ModuleStoreViewProps> = ({
  modules,
  onUpdated,
  onOpenModule,
}) => {
  const [subTab, setSubTab] = useState<"installed" | "browse" | "lists">("installed");
  const [search, setSearch] = useState("");
  const [catFilter, setCatFilter] = useState<string>("all");
  const [listFilter, setListFilter] = useState<string>("all");
  const [sourceInput, setSourceInput] = useState("");
  const [lists, setLists] = useState<CuratedListSubscription[]>([]);
  const [catalogEntries, setCatalogEntries] = useState<CatalogEntry[]>([]);
  const [removedSources, setRemovedSources] = useState<RemovedModuleRecord[]>([]);
  const [updatesById, setUpdatesById] = useState<Record<string, ModuleUpdateInfo>>({});
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [loadingCatalog, setLoadingCatalog] = useState(false);
  const [newListName, setNewListName] = useState("");
  const [newListSource, setNewListSource] = useState("");
  const [showCreateLocal, setShowCreateLocal] = useState(false);
  const [localId, setLocalId] = useState("");
  const [localName, setLocalName] = useState("");
  const [localCat, setLocalCat] = useState<ModuleCategory>("widget");
  const [inspecting, setInspecting] = useState(false);
  const [pendingDisclosure, setPendingDisclosure] = useState<{
    source: string;
    inspectToken: string;
    disclosure: DisclosureReport;
    isUpdate?: boolean;
    migrateFromId?: string;
    changelog?: string | null;
  } | null>(null);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const loadUpdatesAndRemoved = useCallback(async () => {
    setCheckingUpdates(true);
    try {
      const [modRes, updRes] = await Promise.all([
        v2Client.listModules(),
        v2Client.checkModuleUpdates().catch(() => ({ updates: [], available_count: 0 })),
      ]);
      setRemovedSources(modRes.removed_sources ?? []);
      const map: Record<string, ModuleUpdateInfo> = {};
      for (const u of updRes.updates ?? []) {
        if (u.has_update || u.deprecated || u.moved_to || u.replaced_by) map[u.id] = u;
      }
      setUpdatesById(map);
    } finally {
      setCheckingUpdates(false);
    }
  }, []);

  const loadCatalogs = useCallback(async () => {
    setLoadingCatalog(true);
    try {
      const res = await v2Client.listCuratedLists();
      const subs = res.lists ?? [];
      setLists(subs);
      const all: CatalogEntry[] = [];
      for (const sub of subs) {
        try {
          const doc = await v2Client.browseCuratedList(sub.source);
          for (const e of doc.list?.entries ?? []) {
            all.push({
              ...(e as unknown as CatalogEntry),
              listId: sub.id,
              listName: sub.name,
            });
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
    void loadUpdatesAndRemoved();
  }, [loadCatalogs, loadUpdatesAndRemoved]);

  const installedById = useMemo(() => {
    const m = new Map<string, V2ModuleRecord>();
    for (const mod of modules) m.set(mod.id, mod);
    return m;
  }, [modules]);

  const availableUpdateList = useMemo(
    () => Object.values(updatesById).filter((u) => u.has_update || Boolean(u.moved_to)),
    [updatesById],
  );

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
          Number(Boolean(updatesById[b.id])) - Number(Boolean(updatesById[a.id])) ||
          Number(b.state.enabled) - Number(a.state.enabled) ||
          a.manifest.name.localeCompare(b.manifest.name),
      );
  }, [modules, search, catFilter, updatesById]);

  const filteredCatalog = useMemo(() => {
    const q = search.trim().toLowerCase();
    return catalogEntries.filter((e) => {
      // Hide deprecated/abandoned entries from default browse listings unless explicitly searched
      const isRetired = e.deprecated || e.maturity === "deprecated" || e.maturity === "abandoned";
      if (isRetired && !q) return false;
      if (listFilter !== "all" && e.listId !== listFilter) return false;
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
  }, [catalogEntries, search, catFilter, listFilter]);

  const handleToggle = async (mod: V2ModuleRecord) => {
    try {
      await v2Client.updateModuleState(mod.id, { enabled: !mod.state.enabled });
      await onUpdated();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleInspectSource = async (
    src: string,
    opts?: { isUpdate?: boolean; migrateFromId?: string; changelog?: string | null },
  ) => {
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
        isUpdate: opts?.isUpdate,
        migrateFromId: opts?.migrateFromId,
        changelog: opts?.changelog,
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
      if (pendingDisclosure.migrateFromId) {
        const res = await v2Client.migrateModule(
          pendingDisclosure.migrateFromId,
          pendingDisclosure.source,
          true,
          pendingDisclosure.inspectToken,
        );
        setPendingDisclosure(null);
        await onUpdated();
        await loadUpdatesAndRemoved();
        setStatusMsg(
          `Migrated ${res.old_id} → ${res.new_id} (v${res.preview.disclosure.version}) and preserved your settings.`,
        );
        return;
      }
      const res = await v2Client.installModule(
        pendingDisclosure.source,
        true,
        pendingDisclosure.inspectToken,
      );
      const installedId = res.preview.disclosure.module_id;
      const verb = pendingDisclosure.isUpdate ? "Updated" : "Installed & enabled";
      setPendingDisclosure(null);
      setSourceInput("");
      if (!pendingDisclosure.isUpdate) {
        await v2Client.updateModuleState(installedId, { enabled: true }).catch(() => {});
      }
      await onUpdated();
      await loadUpdatesAndRemoved();
      setStatusMsg(`${verb} ${res.preview.disclosure.name} v${res.preview.disclosure.version}`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleAddList = async () => {
    if (!newListSource.trim()) return;
    const name = newListName.trim() || "Custom Module List";
    const id =
      name
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-|-$/g, "") || `list-${Date.now()}`;
    try {
      await v2Client.addCuratedList({ id, name, source: newListSource.trim(), official: false });
      setNewListName("");
      setNewListSource("");
      await loadCatalogs();
      await loadUpdatesAndRemoved();
      setStatusMsg(`Added & synced module list "${name}".`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCreateLocal = async () => {
    const cleanId = localId
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9-]+/g, "-")
      .replace(/^-|-$/g, "");
    if (!cleanId) return;
    try {
      await v2Client.createModule({ id: cleanId, category: localCat, name: localName.trim() || cleanId, enable: true });
      setLocalId("");
      setLocalName("");
      setShowCreateLocal(false);
      await onUpdated();
      setStatusMsg(
        `Created local module "${cleanId}" in ~/.local/share/openflux/v2/modules/local/${cleanId} — edits hot-reload automatically!`,
      );
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleRefreshAll = async () => {
    await Promise.all([loadCatalogs(), loadUpdatesAndRemoved()]);
    setStatusMsg("Synced all module registries and checked for updates.");
  };

  return (
    <div className="kg-module-page">
      <div className="kg-page-header">
        <div className="kg-page-title-row">
          <h1>Manage Modules</h1>
          <span className="kg-pill-subtle">
            {modules.filter((m) => m.state.enabled).length} active · {modules.length} installed ·{" "}
            {catalogEntries.length} in {lists.length} {lists.length === 1 ? "list" : "lists"}
          </span>
          {availableUpdateList.length > 0 && (
            <span className="kg-pill-accent">
              {availableUpdateList.length} {availableUpdateList.length === 1 ? "update" : "updates"} available
            </span>
          )}
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <button
            type="button"
            className="kg-btn-ghost"
            disabled={checkingUpdates || loadingCatalog}
            onClick={() => void handleRefreshAll()}
          >
            {checkingUpdates || loadingCatalog ? "Checking…" : "Check for Updates"}
          </button>
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
      </div>

      {/* TOCTOU-free Permission Disclosure & Update Review Banner */}
      {pendingDisclosure && (
        <div className="kg-calm-card kg-disclosure-box" style={{ marginTop: 0 }}>
          <strong>
            {pendingDisclosure.migrateFromId
              ? "Review Module Migration"
              : pendingDisclosure.isUpdate
                ? "Review Update"
                : "Review Permissions"}
            : {pendingDisclosure.disclosure.name} v{pendingDisclosure.disclosure.version}
          </strong>
          {pendingDisclosure.changelog && (
            <div className="kg-muted" style={{ marginTop: 4, fontSize: 12 }}>
              What’s new: {pendingDisclosure.changelog}
            </div>
          )}
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
              {pendingDisclosure.migrateFromId
                ? "Approve & Migrate"
                : pendingDisclosure.isUpdate
                  ? "Approve & Update"
                  : "Approve & Install"}
            </button>
            <button type="button" className="kg-btn-ghost" onClick={() => setPendingDisclosure(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {statusMsg && (
        <div className="kg-toast-banner" style={{ marginBottom: 0 }}>
          {statusMsg}
        </div>
      )}

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
          {subTab === "browse" && lists.length > 1 && (
            <select
              value={listFilter}
              onChange={(e) => setListFilter(e.target.value)}
              style={{ minWidth: 160 }}
            >
              <option value="all">All Lists ({lists.length})</option>
              {lists.map((l) => (
                <option key={l.id} value={l.id}>
                  {l.name}
                </option>
              ))}
            </select>
          )}
          <input
            type="text"
            style={{ flex: 1, minWidth: 200 }}
            placeholder="Search modules by name, description, or tag…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          {subTab === "installed" && (
            <button
              type="button"
              className="kg-btn-ghost"
              onClick={() => setShowCreateLocal((v) => !v)}
            >
              + New Local Module
            </button>
          )}
        </div>
      )}

      {/* Inline Local Module Scaffold Creator */}
      {subTab === "installed" && showCreateLocal && (
        <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <strong>Create a New Local Module</strong>
          <div className="kg-muted" style={{ fontSize: 12 }}>
            Scaffolds a hot-reloading module under <code>~/.local/share/openflux/v2/modules/local/&lt;id&gt;/</code>.
            Or open any existing module and click <strong>Fork to Local</strong> to customize it.
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <input
              type="text"
              placeholder="module-id (e.g. my-counter)"
              value={localId}
              onChange={(e) => setLocalId(e.target.value)}
              style={{ width: 200 }}
            />
            <input
              type="text"
              placeholder="Display Name"
              value={localName}
              onChange={(e) => setLocalName(e.target.value)}
              style={{ flex: 1, minWidth: 180 }}
            />
            <select value={localCat} onChange={(e) => setLocalCat(e.target.value as ModuleCategory)}>
              <option value="widget">Top-Bar Widget</option>
              <option value="effect">Key Effect</option>
              <option value="background">Background</option>
              <option value="host_service">Host Integration</option>
            </select>
            <button
              type="button"
              className="kg-btn-primary"
              disabled={!localId.trim()}
              onClick={() => void handleCreateLocal()}
            >
              Create
            </button>
          </div>
        </div>
      )}

      {/* TAB 1: Installed (Core + External Installed + Local) */}
      {subTab === "installed" && (
        <>
          <div className="kg-store-grid">
            {filteredInstalled.map((m) => {
              const upd = updatesById[m.id];
              const supportUrl = m.manifest.support_url || m.manifest.issues_url;
              return (
                <div key={m.id} className="kg-calm-card kg-store-item">
                  <div className="kg-calm-row">
                    <div>
                      <strong>{m.manifest.name}</strong>
                      <MaturityBadge maturity={upd?.maturity || m.manifest.maturity} deprecated={m.manifest.deprecated} />{" "}
                      <span className="kg-muted" style={{ fontSize: 11 }}>
                        v{m.manifest.version} · {m.tier === "bundled" ? "Core" : m.tier} ·{" "}
                        {m.manifest.host_requirement === "none" ? "Sandboxed" : "Host Helper"}
                      </span>
                    </div>
                    <div style={{ display: "flex", gap: 6 }}>
                      {upd?.moved_to && (
                        <button
                          type="button"
                          className="kg-btn-primary"
                          disabled={inspecting}
                          onClick={() =>
                            void handleInspectSource(upd.moved_to!, {
                              migrateFromId: m.id,
                              changelog: upd.deprecation_message || `Moved to ${upd.replaced_by || upd.moved_to}`,
                            })
                          }
                        >
                          Migrate{upd.replaced_by ? ` → ${upd.replaced_by}` : ""}
                        </button>
                      )}
                      {upd && upd.has_update && !upd.moved_to && (
                        <button
                          type="button"
                          className="kg-btn-primary"
                          disabled={inspecting}
                          onClick={() =>
                            void handleInspectSource(upd.source, { isUpdate: true, changelog: upd.changelog })
                          }
                        >
                          Update v{upd.current_version} → v{upd.available_version}
                        </button>
                      )}
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
                  {(upd?.deprecated || m.manifest.deprecated || upd?.moved_to || m.manifest.moved_to) && (
                    <div style={{ fontSize: 11, color: "#fbbf24", marginTop: 4 }}>
                      <strong>{upd?.moved_to || m.manifest.moved_to ? "Moved / Replaced:" : "Deprecated:"}</strong>{" "}
                      {upd?.deprecation_message ||
                        m.manifest.deprecation_message ||
                        `Now maintained at ${upd?.moved_to || m.manifest.moved_to}`}
                    </div>
                  )}
                  {upd?.has_update && upd?.changelog && (
                    <div style={{ fontSize: 11, color: "#7dd3fc", marginTop: 4 }}>
                      Update notes (v{upd.available_version}): {upd.changelog}
                    </div>
                  )}
                  <div style={{ display: "flex", gap: 10, flexWrap: "wrap", fontSize: 11, marginTop: 6 }} className="kg-muted">
                    {m.manifest.author && <span>By {m.manifest.author}</span>}
                    {m.manifest.license && <span>{m.manifest.license}</span>}
                    {m.manifest.repository && (
                      <a href={m.manifest.repository} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
                        Source ↗
                      </a>
                    )}
                    {supportUrl && (
                      <a href={supportUrl} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
                        Support ↗
                      </a>
                    )}
                    {m.manifest.discord_url && (
                      <a href={m.manifest.discord_url} target="_blank" rel="noreferrer" style={{ color: "#a5b4fc" }}>
                        Discord ↗
                      </a>
                    )}
                  </div>
                </div>
              );
            })}
          </div>

          {/* Remembered Previously-Removed Modules (including direct installs with no list!) */}
          {removedSources.length > 0 && (
            <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <div className="kg-row-title">Previously Removed Modules</div>
              <div className="kg-muted" style={{ fontSize: 12 }}>
                Keyglass remembers where removed modules came from so you can re-install them with one click — even
                direct installs that aren’t in any registry list.
              </div>
              {removedSources.map((r) => (
                <div key={r.id} className="kg-action-row">
                  <div>
                    <strong>{r.name}</strong>{" "}
                    <span className="kg-muted" style={{ fontSize: 11 }}>
                      last v{r.version}
                    </span>
                    <div className="kg-muted" style={{ fontSize: 11 }}>
                      <code>{r.source}</code>
                    </div>
                  </div>
                  <div style={{ display: "flex", gap: 6 }}>
                    <button
                      type="button"
                      className="kg-btn-primary"
                      disabled={inspecting}
                      onClick={() => void handleInspectSource(r.source)}
                    >
                      Re-install
                    </button>
                    <button
                      type="button"
                      className="kg-btn-ghost"
                      onClick={async () => {
                        await v2Client.forgetRemovedSource(r.id);
                        await loadUpdatesAndRemoved();
                      }}
                    >
                      Forget
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {/* TAB 2: Browse Subscribed Registries (Keyglass Official + Community Taps) */}
      {subTab === "browse" && (
        <>
          {loadingCatalog && catalogEntries.length === 0 ? (
            <div className="kg-muted">Loading subscribed module registries…</div>
          ) : filteredCatalog.length === 0 ? (
            <div className="kg-calm-card kg-muted">
              No modules found in subscribed registries. Add a registry list under{" "}
              <strong>Lists &amp; Direct Install</strong>.
            </div>
          ) : (
            <div className="kg-store-grid">
              {filteredCatalog.map((e) => {
                const existing = installedById.get(e.id);
                const upd = updatesById[e.id];
                return (
                  <div key={`${e.listId}:${e.id}`} className="kg-calm-card kg-store-item">
                    <div className="kg-calm-row">
                      <div>
                        <strong>{e.name}</strong>
                        <MaturityBadge maturity={e.maturity} deprecated={e.deprecated} />{" "}
                        <span className="kg-muted" style={{ fontSize: 11 }}>
                          v{e.version} · {e.listName || "Registry"} ·{" "}
                          {e.runs_host_process ? "Host Helper" : "Sandboxed"}
                        </span>
                      </div>
                      <div style={{ display: "flex", gap: 6 }}>
                        {existing ? (
                          <>
                            {upd && (
                              <button
                                type="button"
                                className="kg-btn-primary"
                                disabled={inspecting}
                                onClick={() =>
                                  void handleInspectSource(e.source, { isUpdate: true, changelog: e.changelog })
                                }
                              >
                                Update v{existing.manifest.version} → v{e.version}
                              </button>
                            )}
                            <button type="button" className="kg-btn-ghost" onClick={() => onOpenModule(existing.id)}>
                              Open
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
                    {e.changelog && (
                      <div className="kg-muted" style={{ fontSize: 11, marginTop: 4 }}>
                        v{e.version}: {e.changelog}
                      </div>
                    )}
                    <div style={{ display: "flex", gap: 10, flexWrap: "wrap", fontSize: 11, marginTop: 6 }} className="kg-muted">
                      {e.author && <span>By {e.author}</span>}
                      {e.license && <span>{e.license}</span>}
                      {e.homepage && (
                        <a href={e.homepage} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
                          Homepage ↗
                        </a>
                      )}
                      {e.repository && (
                        <a href={e.repository} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
                          Source ↗
                        </a>
                      )}
                      {e.support_url && (
                        <a href={e.support_url} target="_blank" rel="noreferrer" style={{ color: "#7dd3fc" }}>
                          Support ↗
                        </a>
                      )}
                      {e.discord_url && (
                        <a href={e.discord_url} target="_blank" rel="noreferrer" style={{ color: "#a5b4fc" }}>
                          Discord ↗
                        </a>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </>
      )}

      {/* TAB 3: Subscribed Module Lists ("Taps") + Direct Install + Local Authoring Guide */}
      {subTab === "lists" && (
        <>
          <div className="kg-calm-card">
            <div className="kg-row-title" style={{ marginBottom: 6 }}>
              Direct Install (No List Required — GitHub Repo, URL, Archive, or Local Folder)
            </div>
            <div className="kg-muted" style={{ fontSize: 12, marginBottom: 8 }}>
              Keyglass tracks updates directly from this location and remembers it under{" "}
              <strong>Previously Removed Modules</strong> if you uninstall it later.
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
            <div className="kg-calm-row">
              <div className="kg-row-title">Subscribed Module Lists (“Taps”)</div>
              <button
                type="button"
                className="kg-btn-ghost"
                disabled={loadingCatalog}
                onClick={() => void handleRefreshAll()}
              >
                {loadingCatalog ? "Syncing…" : "Sync All Lists"}
              </button>
            </div>
            <div className="kg-muted" style={{ fontSize: 12 }}>
              Anyone can publish a static <code>catalog.json</code> on GitHub Pages — subscribe here to browse, install,
              and update their modules right inside Keyglass Studio.
            </div>
            {lists.map((l) => (
              <div key={l.id} className="kg-action-row">
                <div>
                  <strong>{l.name}</strong> {l.official && <span className="kg-pill-accent">Official</span>}
                  <div className="kg-muted" style={{ fontSize: 11 }}>
                    <code>{l.source}</code>
                  </div>
                </div>
                {!l.official && (
                  <button
                    type="button"
                    className="kg-btn-ghost"
                    onClick={async () => {
                      await v2Client.removeCuratedList(l.id);
                      await loadCatalogs();
                      await loadUpdatesAndRemoved();
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
                placeholder="List Name"
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

          <div className="kg-calm-card" style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            <div className="kg-row-title">Developing &amp; Forking Local Modules</div>
            <div className="kg-muted" style={{ fontSize: 12 }}>
              • <strong>Create a new module:</strong> Click <strong>+ New Local Module</strong> under{" "}
              <em>Installed</em> (or run <code>fluxctl modules create my-mod --category widget</code>).
              <br />• <strong>Customize any existing module:</strong> Open the module and click{" "}
              <strong>Fork to Local</strong> (or run <code>fluxctl modules fork &lt;id&gt; &lt;new-id&gt;</code>).
              <br />• <strong>Fast edit loop:</strong> Edit files in{" "}
              <code>~/.local/share/openflux/v2/modules/local/</code> — Keyglass auto-reloads on save,{" "}
              <code>fluxctl perf --watch</code> profiles Lua µs hotspots, and <code>fluxctl mcp</code> connects AI
              coding tools directly to your live keyboard.
            </div>
          </div>
        </>
      )}
    </div>
  );
};
