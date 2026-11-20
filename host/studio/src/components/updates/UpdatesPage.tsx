import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { type DaemonError, toDaemonError } from "../../api/daemonError";
import type { DaemonServiceClient } from "../../api/daemonService";
import type { HostUpdateFlow } from "../../api/hostUpdateFlow";
import {
  COMPONENT_LABELS,
  type ComponentStatus,
  type ConversionStatus,
  type ExpectedPlanStep,
  type Job,
  type KeyboardComponent,
  type KeyboardStatus,
  type PlanStep,
  type UpdateChannel,
  type UpdatePlan,
  type UpdatesClient,
  type UpdatesStatus,
} from "../../api/updates";
import { PressRefinementOutcome } from "../PressRefinementRow";
import { BackgroundServiceCard } from "./BackgroundServiceCard";
import { ConversionWizard, FactoryReset } from "./ConversionWizard";
import { ConfirmDialog, DaemonErrorNotice, ErrorNotice, formatTime, JobProgress, useJob } from "./common";
import { HostUpdateCard } from "./HostUpdateCard";

export function componentStateLabel(c: ComponentStatus): string {
  switch (c.state) {
    case "upToDate":
      return "Up to date";
    case "updateAvailable":
      return c.latest ? `Update available: ${c.latest}` : "Update available";
    case "needsNewerHost":
      return "Needs a newer OpenFlux first";
    case "newerThanHost":
      return "Keyboard newer than this OpenFlux";
    case "conversionRequired":
      return "Conversion required";
    case "blocked":
      return "Update blocked";
    case "incompatible":
      return "Incompatible";
    case "unknown":
      return c.kind === "openflux" ? "Not in the release index" : "Unknown";
  }
}

const INDEX_SOURCE_NOTES: Record<UpdatesStatus["indexSource"], string | null> = {
  online: null,
  bundled: "Showing the releases that came with this installation. Check now to see newer ones.",
  none: "No release information is available yet. Check now to look for updates.",
};

const ComponentTable: React.FC<{ components: ComponentStatus[]; onOpenUrl: (url: string) => void }> = ({
  components,
  onOpenUrl,
}) => (
  <table className="component-table">
    <thead>
      <tr>
        <th>Component</th>
        <th>Installed</th>
        <th>Status</th>
      </tr>
    </thead>
    <tbody>
      {components.map((c) => (
        <tr key={c.component} data-component={c.component} data-state={c.state}>
          <td>{COMPONENT_LABELS[c.component]}</td>
          <td>{c.installed ?? "Not detected"}</td>
          <td>
            <span className={`state-label state-${c.state}`}>{componentStateLabel(c)}</span>
            {c.required && c.state === "updateAvailable" && <span className="pill pill-warn">Required</span>}
            {c.reason && <div className="muted">{c.reason}</div>}
            {c.notesUrl && (
              <button type="button" className="link-btn" onClick={() => onOpenUrl(c.notesUrl as string)}>
                Release notes
              </button>
            )}
          </td>
        </tr>
      ))}
    </tbody>
  </table>
);

/** Keyboard updates of a keyboard that runs OpenFlux: the plan, a confirmation, and the install. */
const KeyboardUpdates: React.FC<{
  updates: UpdatesClient;
  keyboard: KeyboardStatus;
  jobRunning: boolean;
  onJobStarted: (jobId: string) => void;
  onPlanChanged?: () => void;
}> = ({ updates, keyboard, jobRunning, onJobStarted, onPlanChanged }) => {
  const available = keyboard.components.filter((c) => c.state === "updateAvailable");
  const [selected, setSelected] = useState<Set<KeyboardComponent>>(() => new Set(available.map((c) => c.component)));
  const [subsetPlan, setSubsetPlan] = useState<UpdatePlan | null>(null);
  const [planningSubset, setPlanningSubset] = useState(false);
  const [confirmPlan, setConfirmPlan] = useState<{
    components?: KeyboardComponent[];
    steps: PlanStep[];
  } | null>(null);
  const [error, setError] = useState<DaemonError | null>(null);
  const availableKey = available.map((c) => c.component).join(",");
  useEffect(() => {
    setSelected(new Set(availableKey ? (availableKey.split(",") as KeyboardComponent[]) : []));
  }, [availableKey]);

  const allSelected = available.length > 0 && available.every((c) => selected.has(c.component));
  const selectedKey = [...selected].sort().join(",");

  // Preview the exact plan for a partial selection so the user sees (and confirms) what will run.
  useEffect(() => {
    if (allSelected || selected.size === 0 || !updates.planUpdates) {
      setSubsetPlan(null);
      setPlanningSubset(false);
      return;
    }
    let cancelled = false;
    setPlanningSubset(true);
    const comps = selectedKey.split(",") as KeyboardComponent[];
    void updates
      .planUpdates(comps)
      .then((res) => {
        if (!cancelled) {
          setSubsetPlan(res.plan);
          setPlanningSubset(false);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSubsetPlan(null);
          setPlanningSubset(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [allSelected, selected.size, selectedKey, updates, keyboard.plan]);

  const notIndexed = keyboard.components.some((c) => c.kind === "openflux" && c.state === "unknown");
  const activePlan = !allSelected && subsetPlan ? subsetPlan : keyboard.plan;
  const steps = activePlan?.steps ?? [];

  if (notIndexed && !keyboard.plan) {
    return (
      <p className="muted">
        The version on this keyboard isn't in the release index, so OpenFlux can't plan updates for it. Check for
        updates to fetch the newest release information.
      </p>
    );
  }
  if ((keyboard.plan?.steps ?? []).length === 0) {
    return keyboard.plan?.reason ? (
      <p className="muted">{keyboard.plan.reason}</p>
    ) : available.length === 0 ? (
      <p className="muted">The keyboard's software is up to date.</p>
    ) : null;
  }

  const openConfirm = () => {
    setError(null);
    setConfirmPlan({
      components: allSelected ? undefined : [...selected],
      steps: steps.map((s) => ({ ...s })),
    });
  };

  const install = async () => {
    const approved = confirmPlan;
    if (!approved) return;
    setConfirmPlan(null);
    setError(null);
    const expectedSteps: ExpectedPlanStep[] = approved.steps.map((s) => ({ component: s.component, to: s.to }));
    try {
      const { jobId } = await updates.applyUpdates(approved.components, expectedSteps);
      onJobStarted(jobId);
    } catch (e) {
      const err = toDaemonError(e);
      setError(err);
      if (err.code === "planChanged") onPlanChanged?.();
    }
  };

  return (
    <div className="keyboard-install">
      <h4>Install</h4>
      {available.length > 1 && (
        <div className="checkbox-row">
          {available.map((c) => (
            <label key={c.component}>
              <input
                type="checkbox"
                checked={selected.has(c.component)}
                onChange={(e) => {
                  const next = new Set(selected);
                  if (e.target.checked) next.add(c.component);
                  else next.delete(c.component);
                  setSelected(next);
                }}
              />
              {COMPONENT_LABELS[c.component]} {c.latest}
            </label>
          ))}
        </div>
      )}
      <ol className="plan-steps">
        {steps.map((s) => (
          <li key={`${s.component}-${s.to}`}>
            {COMPONENT_LABELS[s.component]} {s.from ? `${s.from} → ` : ""}
            {s.to}
          </li>
        ))}
      </ol>
      {!allSelected && <p className="muted">OpenFlux adds any other component your selection needs.</p>}
      {activePlan?.reason && <p className="muted">{activePlan.reason}</p>}
      <button
        type="button"
        className="btn-primary"
        disabled={jobRunning || selected.size === 0 || planningSubset || steps.length === 0}
        onClick={openConfirm}
      >
        Install keyboard updates…
      </button>
      {error && <DaemonErrorNotice title="The update didn't start" error={error} />}
      {confirmPlan && (
        <ConfirmDialog
          title="Install keyboard updates?"
          confirmLabel="Install"
          onConfirm={() => void install()}
          onCancel={() => setConfirmPlan(null)}
        >
          <ol className="plan-steps">
            {confirmPlan.steps.map((s) => (
              <li key={`${s.component}-${s.to}`}>
                {COMPONENT_LABELS[s.component]} {s.to}
              </li>
            ))}
          </ol>
          <p>
            While the update installs, the keyboard can't type, and its display may go dark or restart. It usually takes
            a few minutes. Keep the keyboard connected until OpenFlux says it's done, and use another keyboard if you
            need to type in the meantime.
          </p>
          <p className="muted">
            OpenFlux saves the keyboard's settings, calibration and current software first, so a step that fails is
            rolled back.
          </p>
        </ConfirmDialog>
      )}
    </div>
  );
};

/** The Updates page: OpenFlux itself, and the keyboard's software. */
export const UpdatesPage: React.FC<{
  updates: UpdatesClient;
  hostFlow: HostUpdateFlow;
  /** Shows whether the background service runs and starts at login. */
  service?: DaemonServiceClient;
  /** Called with every status the page loads, so the navigation badge stays current. */
  onStatus?: (s: UpdatesStatus) => void;
  /** Called after a keyboard job succeeds, so the rest of Studio reloads the keyboard's config. */
  onKeyboardChanged?: () => void;
  /** Poll intervals for a running check and a running job. */
  pollMs?: number;
  jobPollMs?: number;
}> = ({ updates, hostFlow, service, onStatus, onKeyboardChanged, pollMs = 1500, jobPollMs = 1000 }) => {
  const [status, setStatus] = useState<UpdatesStatus | null>(null);
  const [conversion, setConversion] = useState<ConversionStatus | null>(null);
  const [loadError, setLoadError] = useState<DaemonError | null>(null);
  const [actionError, setActionError] = useState<DaemonError | null>(null);
  // A failed poll while a check runs: shown, and the poll tries again.
  const [checkPollError, setCheckPollError] = useState<{ error: DaemonError; attempt: number } | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [finishedJob, setFinishedJob] = useState<Job | null>(null);
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;
  const onChangedRef = useRef(onKeyboardChanged);
  onChangedRef.current = onKeyboardChanged;

  const accept = useCallback((s: UpdatesStatus) => {
    setStatus(s);
    setLoadError(null);
    setCheckPollError(null);
    onStatusRef.current?.(s);
    if (s.job && s.job.state === "running") setJobId(s.job.id);
  }, []);

  const refresh = useCallback(async () => {
    try {
      accept(await updates.getUpdates());
    } catch (e) {
      setLoadError(toDaemonError(e));
    }
    try {
      const c = await updates.getConversion();
      setConversion(c);
      if (c.job && c.job.state === "running") setJobId(c.job.id);
    } catch {
      setConversion(null);
    }
  }, [updates, accept]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Follow a running check until it ends, also through failed polls.
  // biome-ignore lint/correctness/useExhaustiveDependencies: checkPollError schedules the next poll after a failed one.
  useEffect(() => {
    if (!status?.checking) return;
    const t = window.setTimeout(() => {
      void updates
        .getUpdates()
        .then(accept)
        .catch((e) => setCheckPollError((prev) => ({ error: toDaemonError(e), attempt: (prev?.attempt ?? 0) + 1 })));
    }, pollMs);
    return () => window.clearTimeout(t);
  }, [status, checkPollError, updates, accept, pollMs]);

  // When the daemon knows of an OpenFlux update, ask the updater too so it can be installed.
  const hostAvailable = status?.host.state === "updateAvailable";
  const channel = status?.channel;
  useEffect(() => {
    if (hostAvailable && channel && hostFlow.state.phase === "idle") void hostFlow.check(channel);
  }, [hostAvailable, channel, hostFlow]);

  const { job, unreachable } = useJob(
    updates,
    jobId,
    (j) => {
      setFinishedJob(j);
      setJobId(null);
      if (j.state === "succeeded") onChangedRef.current?.();
      void refresh();
    },
    jobPollMs,
  );
  const jobRunning = jobId !== null;

  const checkNow = async () => {
    setActionError(null);
    try {
      accept(await updates.checkUpdates());
    } catch (e) {
      setActionError(toDaemonError(e));
    }
    if (status) void hostFlow.check(status.channel);
  };

  const saveSettings = async (patch: { channel?: UpdateChannel; autoCheck?: boolean }) => {
    setActionError(null);
    try {
      const s = await updates.putUpdateSettings(patch);
      accept(s);
      if (patch.channel) void hostFlow.check(s.channel);
    } catch (e) {
      setActionError(toDaemonError(e));
    }
  };

  const openUrl = (url: string) => void updates.openUrl(url).catch(() => {});
  const startJob = (id: string) => {
    setFinishedJob(null);
    setJobId(id);
  };

  if (!status) {
    return (
      <div className="updates-page">
        {loadError ? (
          <DaemonErrorNotice title="Couldn't load update information" error={loadError} />
        ) : (
          <p className="muted">Loading update information…</p>
        )}
      </div>
    );
  }

  const kb = status.keyboard;
  const sourceNote = INDEX_SOURCE_NOTES[status.indexSource];
  return (
    <div className="updates-page">
      <div className="card updates-header">
        <div className="toolbar-between">
          <div>
            <h3>Updates</h3>
            <p className="muted">
              {status.checking ? "Checking for updates…" : `Last checked: ${formatTime(status.checkedAt)}`}
            </p>
          </div>
          <button type="button" className="btn-primary" disabled={status.checking} onClick={() => void checkNow()}>
            {status.checking ? "Checking…" : "Check now"}
          </button>
        </div>
        <div className="row-2">
          <label className="field">
            <span>Update channel</span>
            <select
              aria-label="Update channel"
              value={status.channel}
              onChange={(e) => void saveSettings({ channel: e.target.value as UpdateChannel })}
            >
              <option value="stable">Stable</option>
              <option value="beta">Beta (early releases, may have problems)</option>
            </select>
          </label>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={status.autoCheck}
              onChange={(e) => void saveSettings({ autoCheck: e.target.checked })}
            />
            Check for updates automatically
          </label>
        </div>
        {sourceNote && <p className="muted">{sourceNote}</p>}
        {status.error && (
          <ErrorNotice title="The last check failed" code={status.error.code} message={status.error.message} />
        )}
        {status.checking && checkPollError && (
          <DaemonErrorNotice title="Couldn't follow the running check. Trying again…" error={checkPollError.error} />
        )}
        {actionError && <DaemonErrorNotice error={actionError} />}
      </div>

      <HostUpdateCard flow={hostFlow} host={status.host} onOpenUrl={openUrl} />
      {service && <BackgroundServiceCard service={service} />}

      <section className="card" aria-label="Keyboard">
        <h3>Keyboard</h3>
        {(jobRunning || finishedJob) && (
          <>
            <JobProgress job={jobRunning ? job : finishedJob} unreachable={unreachable} />
            {finishedJob?.state === "succeeded" && finishedJob.kind === "convert" && (
              <>
                <p>The keyboard runs OpenFlux, and your settings were imported.</p>
                <PressRefinementOutcome outcome={finishedJob.pressRefinement} />
              </>
            )}
            {finishedJob && (
              <button type="button" className="btn-secondary" onClick={() => setFinishedJob(null)}>
                Dismiss
              </button>
            )}
          </>
        )}
        {kb.mode === "disconnected" ? (
          <p className="muted">Connect the keyboard to see and install its updates.</p>
        ) : (
          <ComponentTable components={kb.components} onOpenUrl={openUrl} />
        )}
        {kb.mode === "needsConversion" && (
          <ConversionWizard
            updates={updates}
            keyboard={kb}
            conversion={conversion}
            jobRunning={jobRunning}
            onJobStarted={startJob}
          />
        )}
        {kb.mode === "openflux" && (
          <KeyboardUpdates
            updates={updates}
            keyboard={kb}
            jobRunning={jobRunning}
            onJobStarted={startJob}
            onPlanChanged={() => void refresh()}
          />
        )}
        {kb.mode === "openflux" && (
          <details className="advanced">
            <summary>Advanced</summary>
            <FactoryReset updates={updates} conversion={conversion} jobRunning={jobRunning} onJobStarted={startJob} />
          </details>
        )}
      </section>
    </div>
  );
};
