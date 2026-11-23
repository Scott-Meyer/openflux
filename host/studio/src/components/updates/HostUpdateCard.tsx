import type React from "react";
import { useEffect, useState } from "react";
import type { HostUpdateFlow, HostUpdateState } from "../../api/hostUpdateFlow";
import type { HostStatus } from "../../api/updates";
import { DaemonErrorNotice } from "./common";

function useFlowState(flow: HostUpdateFlow): HostUpdateState {
  const [s, setS] = useState(flow.state);
  useEffect(() => {
    setS(flow.state);
    return flow.subscribe(setS);
  }, [flow]);
  return s;
}

function formatBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${Math.round(n / 1024)} KB`;
}

const FAILED_TITLES = {
  check: "Couldn't check for a Keyglass update",
  download: "The update couldn't be downloaded",
  stop: "The background service couldn't be stopped for the update",
  install: "The update couldn't be installed",
} as const;

/** Keyglass's own update: the daemon's view of it, and Studio's updater. */
export const HostUpdateCard: React.FC<{
  flow: HostUpdateFlow;
  host: HostStatus | null;
  onOpenUrl: (url: string) => void;
}> = ({ flow, host, onOpenUrl }) => {
  const s = useFlowState(flow);
  const notes = host?.notesUrl ? (
    <button type="button" className="link-btn" onClick={() => onOpenUrl(host.notesUrl as string)}>
      Release notes
    </button>
  ) : null;

  const body = (() => {
    switch (s.phase) {
      case "idle":
        return null;
      case "checking":
        return <p className="muted">Checking for a Keyglass update…</p>;
      case "unsupported":
        return (
          <>
            <p className="muted">{s.reason}</p>
            {host?.state === "updateAvailable" && host.latest && (
              <p>
                Keyglass {host.latest} is available. {notes}
              </p>
            )}
          </>
        );
      case "upToDate":
        return <p>Keyglass {s.currentVersion} is up to date.</p>;
      case "available":
        return (
          <>
            <p>
              Keyglass {s.update.version} is available. You have {s.update.currentVersion}.
            </p>
            {s.update.notes && <p className="muted pre-line">{s.update.notes}</p>}
            <p className="muted">
              Studio restarts to finish. If Keyglass is working on the keyboard, the update waits until that work is
              done.
            </p>
            <div className="btn-group">
              <button type="button" className="btn-primary" onClick={() => void flow.install()}>
                Download and install
              </button>
              <button type="button" className="btn-secondary" onClick={() => flow.cancel()}>
                Not now
              </button>
            </div>
          </>
        );
      case "downloading": {
        const pct = s.total ? Math.round((s.downloaded / s.total) * 100) : null;
        return (
          <>
            <p>
              Downloading Keyglass {s.update.version}
              {s.downloaded > 0 && ` (${formatBytes(s.downloaded)}${s.total ? ` of ${formatBytes(s.total)}` : ""})`}…
            </p>
            <div
              className={`progress-track ${pct == null ? "indeterminate" : ""}`}
              role="progressbar"
              aria-label="Download progress"
              aria-valuenow={pct ?? undefined}
            >
              <div className="progress-fill" style={{ width: pct == null ? "35%" : `${pct}%` }} />
            </div>
            <button type="button" className="btn-secondary" onClick={() => flow.cancel()}>
              Cancel
            </button>
          </>
        );
      }
      case "stoppingDaemon":
        return <p>Stopping the background service for the update…</p>;
      case "waitingForKeyboard": {
        const what = s.job?.step?.label ?? (s.busy.length ? s.busy.join(", ") : "keyboard work");
        return (
          <>
            <p role="status">Waiting for “{what}” to finish.</p>
            <p className="muted">
              {s.cancelling
                ? "The update is cancelled. The background service starts again once this work is done."
                : "The background service finishes its current work on the keyboard and takes no new work. The update installs right after."}
            </p>
            {!s.cancelling && (
              <button type="button" className="btn-secondary" onClick={() => flow.cancel()}>
                Cancel update
              </button>
            )}
          </>
        );
      }
      case "installing":
        return <p>Installing Keyglass {s.update.version}…</p>;
      case "relaunching":
        return <p>Restarting Keyglass Studio…</p>;
      case "cancelled":
        return (
          <>
            <p>The update was cancelled.</p>
            <p className="muted">
              {s.daemonRestarted === null
                ? "Nothing was changed."
                : s.daemonRestarted
                  ? "The background service was started again."
                  : "The background service couldn't be started again. Quit Keyglass Studio and open it again."}
            </p>
          </>
        );
      case "failed":
        return (
          <>
            <DaemonErrorNotice title={FAILED_TITLES[s.stage]} error={s.error} />
            {s.stage !== "install" && <p className="muted">Nothing was installed.</p>}
            {s.daemonRestarted === true && <p className="muted">The background service was started again.</p>}
            {s.daemonRestarted === false && (
              <p className="muted">
                The background service couldn't be started again. Quit Keyglass Studio and open it again.
              </p>
            )}
            {s.update && (
              <button type="button" className="btn-secondary" onClick={() => void flow.install()}>
                Try again
              </button>
            )}
          </>
        );
    }
  })();

  return (
    <section className="card" aria-label="Keyglass app">
      <div className="toolbar-between">
        <h3>Keyglass</h3>
        {host && <span className="pill">Installed {host.installed}</span>}
      </div>
      {host?.state === "updateAvailable" && s.phase === "idle" && (
        <p>
          Keyglass {host.latest} is available. {notes}
        </p>
      )}
      {host?.state === "blocked" && <p className="muted">{host.reason ?? "This update can't be installed."}</p>}
      {body}
    </section>
  );
};
