import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toDaemonError } from "../api/daemonError";
import {
  type DaemonServiceClient,
  type ServiceInspection,
  type ServiceStatus,
  STEP_LABELS,
  type StartReport,
} from "../api/daemonService";
import { OpenFluxLogo } from "./OpenFluxLogo";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

type Phase =
  | { phase: "probing" }
  | { phase: "waiting" }
  | { phase: "starting"; report: StartReport | null }
  | { phase: "needsApproval"; status: ServiceStatus }
  | {
      phase: "failed";
      reason: string;
      report: StartReport | null;
      service: ServiceInspection | null;
      /** The service is known to be stopped, rather than not answering. */
      notRunning: boolean;
    };

export function registrationLabel(s: ServiceInspection): string {
  if (s.manager === "launchAgent") {
    switch (s.registration) {
      case "enabled":
        return "Login item: on";
      case "requiresApproval":
        return "Login item: needs approval in System Settings";
      case "notRegistered":
        return "Login item: not registered";
      case "notFound":
        return "Login item: not part of this installation";
      default:
        return "Login item: unknown";
    }
  }
  if (s.manager === "runKey") return s.registration === "enabled" ? "Starts at sign-in" : "Not set to start at sign-in";
  if (s.manager === "systemdUser")
    return s.registration === "enabled" ? "systemd user service: enabled" : "systemd user service: disabled";
  return "Started by Keyglass Studio";
}

// Documents/flux/config is where apps written for the Flux keyboard's third-party API look for it.
const DocumentsNote: React.FC = () => (
  <p className="muted">
    The first time the background service runs, macOS may ask whether Keyglass can access your Documents folder.
    Keyglass keeps a small file in Documents/flux/config so other apps on this computer can find it. Allow access, or
    those apps can't connect to the keyboard.
  </p>
);

/**
 * Shown before Studio's main window while the background service isn't answering: starts it,
 * or explains what the user needs to do.
 */
export const DaemonStartup: React.FC<{
  service: DaemonServiceClient;
  onReady: (status: ServiceStatus | null) => void;
  pollMs?: number;
  timeoutMs?: number;
}> = ({ service, onReady, pollMs = 1000, timeoutMs = 30_000 }) => {
  const [p, setP] = useState<Phase>({ phase: "probing" });
  const runRef = useRef(0);
  const readyRef = useRef(onReady);
  readyRef.current = onReady;

  const run = useCallback(
    async (mode: "auto" | "direct" | "none") => {
      const attempt = ++runRef.current;
      const live = () => attempt === runRef.current;
      let status: ServiceStatus;
      try {
        status = await service.status();
      } catch (e) {
        if (live())
          setP({ phase: "failed", reason: toDaemonError(e).message, report: null, service: null, notRunning: false });
        return;
      }
      if (!live()) return;
      if (status.reachable) {
        readyRef.current(status);
        return;
      }
      // A daemon that may still be running (starting, or not answering) is waited for; starting
      // another one wouldn't help.
      const mayBeRunning = !status.notRunning && mode !== "direct";
      if (mayBeRunning) setP({ phase: "waiting" });
      let report: StartReport | null = null;
      if (
        !mayBeRunning &&
        mode !== "none" &&
        !(mode === "auto" && status.service.registration === "requiresApproval")
      ) {
        setP({ phase: "starting", report: null });
        try {
          report = await service.start(mode);
        } catch (e) {
          if (live())
            setP({
              phase: "failed",
              reason: toDaemonError(e).message,
              report: null,
              service: status.service,
              notRunning: status.notRunning,
            });
          return;
        }
        if (!live()) return;
        setP({ phase: "starting", report });
      }
      // Approval can take as long as the user needs, so it isn't timed; once it is given, the
      // service gets the usual time to start.
      let awaitingApproval = mode !== "direct" && (report?.after ?? status.service).registration === "requiresApproval";
      if (awaitingApproval) setP({ phase: "needsApproval", status });
      let deadline = Date.now() + timeoutMs;
      let lastError: string | null = null;
      for (;;) {
        await sleep(pollMs);
        if (!live()) return;
        let fresh: ServiceStatus | null = null;
        try {
          fresh = await service.status();
          lastError = null;
        } catch (e) {
          lastError = toDaemonError(e).message;
        }
        if (!live()) return;
        if (fresh) {
          status = fresh;
          if (status.reachable) {
            readyRef.current(status);
            return;
          }
          if (!mayBeRunning && mode !== "direct" && status.service.registration === "requiresApproval") {
            awaitingApproval = true;
            setP({ phase: "needsApproval", status });
            continue;
          }
          if (awaitingApproval) {
            awaitingApproval = false;
            deadline = Date.now() + timeoutMs;
            setP({ phase: "waiting" });
          }
        } else if (awaitingApproval && !mayBeRunning) {
          continue;
        }
        if (Date.now() > deadline) {
          setP({
            phase: "failed",
            reason: (fresh ? status.unreachableReason : lastError) ?? "The background service didn't start.",
            report,
            service: status.service,
            notRunning: fresh?.notRunning ?? false,
          });
          return;
        }
      }
    },
    [service, pollMs, timeoutMs],
  );

  useEffect(() => {
    void run("auto");
    return () => {
      runRef.current++;
    };
  }, [run]);

  const steps = (report: StartReport | null) =>
    report && report.steps.length > 0 ? (
      <ul className="startup-steps">
        {report.steps.map((s) => (
          <li key={s.step} className={s.ok ? "ok" : "failed"}>
            {STEP_LABELS[s.step]}
            {s.ok ? "" : `: ${s.detail ?? "failed"}`}
          </li>
        ))}
      </ul>
    ) : null;

  const isMac = (s: ServiceInspection | null | undefined) => s?.platform === "macos";

  return (
    <main className="startup-screen">
      <div className="card startup-card">
        <div className="brand">
          <OpenFluxLogo size={28} />
          <strong>Keyglass Studio</strong>
        </div>
        {p.phase === "probing" && <p aria-live="polite">Connecting to the Keyglass background service…</p>}
        {p.phase === "waiting" && <p aria-live="polite">Waiting for the Keyglass background service to answer…</p>}
        {p.phase === "starting" && (
          <>
            <p aria-live="polite">Starting the Keyglass background service…</p>
            <div className="progress-track indeterminate" role="progressbar" aria-label="Starting">
              <div className="progress-fill" style={{ width: "35%" }} />
            </div>
            {steps(p.report)}
            {p.report && <p className="muted">{registrationLabel(p.report.after)}</p>}
            {isMac(p.report?.after) && <DocumentsNote />}
          </>
        )}
        {p.phase === "needsApproval" && (
          <>
            <p>
              The OpenFlux background service is turned off in System Settings. Turn on Keyglass Studio under General
              &gt; Login Items &gt; Allow in the Background. Studio continues as soon as it's allowed.
            </p>
            <div className="btn-group">
              <button
                type="button"
                className="btn-primary"
                onClick={() => void service.openLoginItems().catch(() => {})}
              >
                Open Login Items
              </button>
              <button type="button" className="btn-secondary" onClick={() => void run("direct")}>
                Start for this session only
              </button>
            </div>
            <DocumentsNote />
          </>
        )}
        {p.phase === "failed" && (
          <>
            <div className="notice notice-error" role="alert">
              <strong>
                {p.notRunning
                  ? "The OpenFlux background service isn't running."
                  : "Couldn't connect to the Keyglass background service."}
              </strong>
              <p>{p.reason}</p>
            </div>
            {steps(p.report)}
            {p.service && <p className="muted">{registrationLabel(p.service)}</p>}
            <p className="muted">
              Studio needs it to talk to the keyboard. Try again, or continue and Studio connects when it's running.
            </p>
            <div className="btn-group">
              <button type="button" className="btn-primary" onClick={() => void run("auto")}>
                Try again
              </button>
              <button type="button" className="btn-secondary" onClick={() => readyRef.current(null)}>
                Continue without it
              </button>
            </div>
          </>
        )}
      </div>
    </main>
  );
};
