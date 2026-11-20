import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { type DaemonError, toDaemonError } from "../api/daemonError";
import type { Registrations, ThirdPartyClient, ThirdPartySettings } from "../api/thirdParty";
import { DaemonErrorNotice } from "./updates/common";

const PERMISSIONS: { key: keyof ThirdPartySettings; label: string; hint: string }[] = [
  { key: "saveToKeyboard", label: "Save to the keyboard", hint: "Write the current settings to the keyboard." },
  { key: "import", label: "Import settings", hint: "Import setting files into OpenFlux." },
  { key: "profileChange", label: "Switch profiles", hint: "Change the active profile." },
  { key: "shortcutChange", label: "Switch shortcut sets", hint: "Show the shortcuts for the app in use." },
  { key: "mosaicChange", label: "Add and update modules", hint: "Create and update modules on the display." },
];

function requestedAgo(unixSeconds: number, now: number): string {
  const s = Math.max(0, Math.round(now / 1000 - unixSeconds));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  return m === 1 ? "1 minute ago" : `${m} minutes ago`;
}

/** Apps on this computer that use the OpenFlux API: approvals, access and permissions. */
export const ThirdPartyAppsPage: React.FC<{
  client: ThirdPartyClient;
  onPendingCount?: (n: number) => void;
  pollMs?: number;
  platform?: string;
}> = ({ client, onPendingCount, pollMs = 3000, platform }) => {
  const [regs, setRegs] = useState<Registrations | null>(null);
  const [settings, setSettings] = useState<ThirdPartySettings | null>(null);
  const [error, setError] = useState<DaemonError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null);
  // One permission write at a time; the controls wait for it, so each answer describes the
  // settings shown.
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);

  const loadRegs = useCallback(async () => {
    try {
      const r = await client.getRegistrations();
      setRegs(r);
      onPendingCount?.(r.pending.length);
    } catch (e) {
      setError(toDaemonError(e));
    }
  }, [client, onPendingCount]);

  useEffect(() => {
    void loadRegs();
    void client
      .getSettings()
      .then(setSettings)
      .catch((e) => setError(toDaemonError(e)));
    // A registering app waits for an answer, so keep the pending list fresh.
    const id = window.setInterval(() => void loadRegs(), pollMs);
    return () => window.clearInterval(id);
  }, [client, loadRegs, pollMs]);

  const act = async (fn: () => Promise<{ message: string }>) => {
    setError(null);
    try {
      const r = await fn();
      setNotice(r.message);
    } catch (e) {
      setError(toDaemonError(e));
    }
    await loadRegs();
  };

  const putSetting = async (patch: Partial<ThirdPartySettings>) => {
    if (!settings || savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setError(null);
    const previous = settings;
    setSettings({ ...settings, ...patch });
    try {
      const r = await client.putSettings(patch);
      setSettings(r.settings);
      setWarnings(r.discoveryWriteFailures ?? []);
    } catch (e) {
      setError(toDaemonError(e));
      // The write may have been applied before it failed, so show what the daemon has.
      setSettings(await client.getSettings().catch(() => previous));
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const now = Date.now();
  return (
    <div className="panel-grid apps-page">
      <section className="card" aria-label="App access">
        <h3>Apps</h3>
        <p className="muted">
          Apps on this computer can change what the keyboard shows and which profile it uses through the OpenFlux API.
          An app asks once; you decide whether it may.
        </p>
        {error && <DaemonErrorNotice error={error} />}
        {notice && <p className="status-msg">{notice}</p>}

        <h4>Waiting for approval</h4>
        {regs && regs.pending.length === 0 && <p className="muted">No app is asking for access.</p>}
        <ul className="app-list">
          {regs?.pending.map((p) => (
            <li key={p.id} data-pending={p.app}>
              <div>
                <strong>{p.app}</strong>
                <div className="muted">Asked {requestedAgo(p.requestedAt, now)}</div>
              </div>
              <div className="btn-group">
                <button
                  type="button"
                  className="btn-primary"
                  onClick={() => void act(() => client.decide(p.id, "approve"))}
                >
                  Allow
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => void act(() => client.decide(p.id, "deny"))}
                >
                  Deny
                </button>
              </div>
            </li>
          ))}
        </ul>

        <h4>Allowed</h4>
        {regs && regs.approved.length === 0 && <p className="muted">No app has access.</p>}
        <ul className="app-list">
          {regs?.approved.map((a) => (
            <li key={a.app} data-approved={a.app}>
              <strong>{a.app}</strong>
              <div className="btn-group">
                <button type="button" className="btn-secondary" onClick={() => setConfirmRevoke(a.app)}>
                  Remove access…
                </button>
              </div>
            </li>
          ))}
        </ul>

        <h4>Denied</h4>
        {regs && regs.denied.length === 0 && <p className="muted">No app is denied.</p>}
        <ul className="app-list">
          {regs?.denied.map((d) => (
            <li key={d.app} data-denied={d.app}>
              <strong>{d.app}</strong>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => void act(() => client.forgetDenied(d.app))}
              >
                Let it ask again
              </button>
            </li>
          ))}
        </ul>
      </section>

      <section className="card" aria-label="App permissions">
        <h3>Permissions</h3>
        {settings ? (
          <>
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={settings.apiActive}
                disabled={saving}
                onChange={(e) => void putSetting({ apiActive: e.target.checked })}
              />
              Allow apps to use the OpenFlux API
            </label>
            <p className="muted">When this is off, no app can connect, including allowed ones.</p>
            <fieldset className="permission-list" disabled={!settings.apiActive || saving}>
              <legend className="muted">Allowed apps may:</legend>
              {PERMISSIONS.map((p) => (
                <label key={p.key} className="checkbox-row">
                  <input
                    type="checkbox"
                    checked={Boolean(settings[p.key])}
                    onChange={(e) => void putSetting({ [p.key]: e.target.checked })}
                  />
                  <span>
                    {p.label}
                    <span className="muted"> · {p.hint}</span>
                  </span>
                </label>
              ))}
            </fieldset>
            {warnings.length > 0 && (
              <div className="notice notice-warn" role="alert">
                <p>Apps may not see this change: OpenFlux couldn't update these files.</p>
                <ul>
                  {warnings.map((w) => (
                    <li key={w}>{w}</li>
                  ))}
                </ul>
              </div>
            )}
          </>
        ) : (
          <p className="muted">Loading…</p>
        )}
        {platform === "macos" && (
          <p className="muted">
            Apps find OpenFlux through a file in Documents/flux/config. The first time OpenFlux writes it, macOS asks
            whether OpenFlux may access the Documents folder; allow it so apps can connect.
          </p>
        )}
      </section>

      {confirmRevoke && (
        <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Remove access">
          <div className="modal-card confirm-card">
            <h3>Remove access for {confirmRevoke}?</h3>
            <p>Removing access disconnects the app. It can ask again unless you also block it.</p>
            <div className="confirm-actions">
              <button type="button" className="btn-secondary" onClick={() => setConfirmRevoke(null)}>
                Cancel
              </button>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => {
                  const app = confirmRevoke;
                  setConfirmRevoke(null);
                  void act(() => client.revoke(app, false));
                }}
              >
                Remove access
              </button>
              <button
                type="button"
                className="btn-danger"
                onClick={() => {
                  const app = confirmRevoke;
                  setConfirmRevoke(null);
                  void act(() => client.revoke(app, true));
                }}
              >
                Remove and block
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
