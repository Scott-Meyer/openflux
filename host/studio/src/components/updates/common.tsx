import type React from "react";
import { useEffect, useRef, useState } from "react";
import { type DaemonError, explainError } from "../../api/daemonError";
import type { Job, UpdatesClient } from "../../api/updates";

/** A modal asking the user to confirm an action. */
export const ConfirmDialog: React.FC<{
  title: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  children: React.ReactNode;
}> = ({ title, confirmLabel, danger, onConfirm, onCancel, children }) => (
  <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label={title}>
    <div className="modal-card confirm-card">
      <h3>{title}</h3>
      <div className="confirm-body">{children}</div>
      <div className="confirm-actions">
        <button type="button" className="btn-secondary" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className={danger ? "btn-danger" : "btn-primary"} onClick={onConfirm}>
          {confirmLabel}
        </button>
      </div>
    </div>
  </div>
);

/** A failure, explained, with what to do next. */
export const ErrorNotice: React.FC<{
  title?: string;
  code?: string | null;
  message?: string | null;
  recovery?: string | null;
}> = ({ title, code, message, recovery }) => {
  const e = explainError(code, message, recovery);
  return (
    <div className="notice notice-error" role="alert">
      {title && <strong>{title}</strong>}
      <p>{e.message}</p>
      {e.recovery && <p className="notice-recovery">{e.recovery}</p>}
      {e.detail && <p className="muted">Details: {e.detail}</p>}
    </div>
  );
};

export const DaemonErrorNotice: React.FC<{ title?: string; error: DaemonError }> = ({ title, error }) => (
  <ErrorNotice title={title} code={error.code} message={error.message} recovery={error.recovery} />
);

const JOB_TITLES: Record<Job["kind"], string> = {
  update: "Installing keyboard updates",
  convert: "Converting the keyboard to OpenFlux",
  restore: "Restoring the factory software",
};

/**
 * Follows a job until it ends. Jobs keep running in the daemon when Studio closes, so a job
 * found running when a page opens is followed the same way.
 */
export function useJob(
  client: UpdatesClient,
  jobId: string | null,
  onFinished?: (job: Job) => void,
  pollMs = 1000,
): { job: Job | null; unreachable: boolean } {
  const [job, setJob] = useState<Job | null>(null);
  const [unreachable, setUnreachable] = useState(false);
  const finishedRef = useRef(onFinished);
  finishedRef.current = onFinished;

  useEffect(() => {
    setJob(null);
    setUnreachable(false);
    if (!jobId) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      try {
        const j = await client.getJob(jobId);
        if (cancelled) return;
        setJob(j);
        setUnreachable(false);
        if (j.state !== "running") {
          finishedRef.current?.(j);
          return;
        }
      } catch {
        // The daemon may be restarting; its journal keeps the job, so keep asking.
        if (cancelled) return;
        setUnreachable(true);
      }
      timer = window.setTimeout(() => void tick(), pollMs);
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [client, jobId, pollMs]);

  return { job, unreachable };
}

export const JobProgress: React.FC<{ job: Job | null; unreachable?: boolean; title?: string }> = ({
  job,
  unreachable,
  title,
}) => {
  if (!job) {
    return (
      <div className="job-card" aria-live="polite">
        <p className="muted">Starting…</p>
      </div>
    );
  }
  const pct = job.progress == null ? null : Math.round(Math.max(0, Math.min(1, job.progress)) * 100);
  return (
    <div className="job-card" aria-live="polite" data-job-state={job.state}>
      <div className="toolbar-between">
        <strong>{title ?? JOB_TITLES[job.kind]}</strong>
        <span className="pill">
          {job.state === "running" ? "In progress" : job.state === "succeeded" ? "Done" : "Failed"}
        </span>
      </div>
      {job.step && (
        <p className="job-step">
          Step {job.step.index} of {job.step.count}: {job.step.label}
        </p>
      )}
      {job.state === "running" && (
        <div
          className={`progress-track ${pct == null ? "indeterminate" : ""}`}
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct ?? undefined}
          aria-label={job.step?.label ?? "Progress"}
        >
          <div className="progress-fill" style={{ width: pct == null ? "35%" : `${pct}%` }} />
        </div>
      )}
      {job.state === "running" && (
        <p className="muted">Keep the keyboard connected until this finishes. Closing Studio doesn't stop it.</p>
      )}
      {unreachable && job.state === "running" && <p className="muted">Waiting for the background service to answer…</p>}
      {job.state === "failed" && job.error && (
        <ErrorNotice code={job.error.code} message={job.error.message} recovery={job.error.recovery} />
      )}
      {job.log.length > 0 && (
        <details className="job-log">
          <summary>Details</summary>
          <pre>{job.log.join("\n")}</pre>
        </details>
      )}
    </div>
  );
};

export function formatTime(iso: string | null | undefined): string {
  if (!iso) return "never";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}
