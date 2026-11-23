import type React from "react";
import { useEffect, useState } from "react";
import type { DaemonServiceClient, ServiceStatus } from "../../api/daemonService";
import { registrationLabel } from "../DaemonStartup";

/** Whether the background service runs and how it starts at login. */
export const BackgroundServiceCard: React.FC<{ service: DaemonServiceClient }> = ({ service }) => {
  const [status, setStatus] = useState<ServiceStatus | null>(null);
  useEffect(() => {
    let cancelled = false;
    void service
      .status()
      .then((s) => !cancelled && setStatus(s))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [service]);
  if (!status) return null;
  const svc = status.service;
  return (
    <section className="card" aria-label="Background service">
      <h3>Background service</h3>
      <p>{status.reachable ? "Running" : status.notRunning ? "Not running" : "Not responding"}</p>
      <p className="muted">{registrationLabel(svc)}</p>
      {svc.registration === "requiresApproval" && (
        <>
          <p className="muted">
            It won't start at login until Keyglass Studio (may appear as OpenFlux Studio) is allowed under System
            Settings &gt; General &gt; Login Items.
          </p>
          <button type="button" className="btn-secondary" onClick={() => void service.openLoginItems().catch(() => {})}>
            Open Login Items
          </button>
        </>
      )}
    </section>
  );
};
