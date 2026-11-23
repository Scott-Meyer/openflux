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
  type LocalArtifact,
  type LocalScan,
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
      return "Needs a newer Keyglass first";
    case "newerThanHost":
      return "Keyboard newer than this Keyglass";
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
            {c.customBuild && <span className="pill">Custom / local</span>}
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

/** Keyboard updates of a keyboard that runs Keyglass: the plan, a confirmation, and the install. */
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
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyboard.plan plans the selection again when the daemon's full plan changes.
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
        The version on this keyboard isn't in the release index, so Keyglass can't plan updates for it. Check for
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
      {!allSelected && <p className="muted">Keyglass adds any other component your selection needs.</p>}
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
            a few minutes. Keep the keyboard connected until Keyglass says it's done, and use another keyboard if you
            need to type in the meantime.
          </p>
          <p className="muted">
            Keyglass saves the keyboard's settings, calibration and current software first, so a step that fails is
            rolled back.
          </p>
        </ConfirmDialog>
      )}
    </div>
  );
};

function formatSizeKb(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

const LOCAL_COMPONENTS: KeyboardComponent[] = ["firmware", "bridge", "display"];

/** Unsigned local developer builds scanned from the user's chosen local folder. */
const LocalKeyboardUpdates: React.FC<{
  updates: UpdatesClient;
  localBuilds: LocalScan | null | undefined;
  connected: boolean;
  jobRunning: boolean;
  onJobStarted: (jobId: string) => void;
}> = ({ updates, localBuilds, connected, jobRunning, onJobStarted }) => {
  const found: LocalArtifact[] = LOCAL_COMPONENTS.map((c) => localBuilds?.[c] ?? null).filter(
    (a): a is LocalArtifact => a !== null,
  );
  const foundKey = found.map((a) => `${a.component}:${a.path}`).join("|");
  const [selected, setSelected] = useState<Set<KeyboardComponent>>(() => new Set(found.map((a) => a.component)));
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [error, setError] = useState<DaemonError | null>(null);

  useEffect(() => {
    setSelected(new Set(foundKey ? foundKey.split("|").map((s) => s.split(":")[0] as KeyboardComponent) : []));
  }, [foundKey]);

  if (!localBuilds) {
    return (
      <p className="muted">
        Choose a local build folder above (e.g. a Keyglass checkout or output directory) to scan for local{" "}
        <code>.uf2</code> and <code>.ipk</code> builds.
      </p>
    );
  }
  if (!localBuilds.exists) {
    return (
      <ErrorNotice
        title="Local build folder isn't available"
        code="badRequest"
        message={localBuilds.error ?? `Folder not found: ${localBuilds.dir}`}
      />
    );
  }

  const chosen = found.filter((a) => selected.has(a.component));
  const toggle = (c: KeyboardComponent) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(c)) next.delete(c);
      else next.add(c);
      return next;
    });
  };

  const install = async () => {
    if (!updates.installLocalUpdates || chosen.length === 0) return;
    setConfirmOpen(false);
    setError(null);
    try {
      const { jobId } = await updates.installLocalUpdates(chosen.map((a) => a.component));
      onJobStarted(jobId);
    } catch (e) {
      setError(toDaemonError(e));
    }
  };

  return (
    <div className="updates-plan">
      <div className="toolbar-between">
        <h4>Local builds in folder</h4>
        <span className="pill pill-warn">Local (unsigned)</span>
      </div>
      <div className="updates-choices">
        {LOCAL_COMPONENTS.map((c) => {
          const art = localBuilds[c];
          if (!art) {
            return (
              <div key={c} className="muted">
                {COMPONENT_LABELS[c]}: no matching build found in folder
              </div>
            );
          }
          return (
            <label key={c} className="checkbox-row">
              <input type="checkbox" checked={selected.has(c)} disabled={jobRunning} onChange={() => toggle(c)} />
              <span>
                <strong>{COMPONENT_LABELS[c]}</strong>: <code>{art.name}</code>
                {art.version ? ` (${art.version})` : ""} · {formatSizeKb(art.sizeBytes)} · {formatTime(art.modifiedAt)}
              </span>
            </label>
          );
        })}
      </div>
      {found.length > 0 && (
        <div className="button-row">
          <button
            type="button"
            className="btn-primary"
            disabled={jobRunning || !connected || chosen.length === 0 || !updates.installLocalUpdates}
            onClick={() => setConfirmOpen(true)}
          >
            Install local builds…
          </button>
          {!connected && <span className="muted">Connect the keyboard to install local builds.</span>}
        </div>
      )}
      {error && <DaemonErrorNotice error={error} />}
      {confirmOpen && (
        <ConfirmDialog
          title="Install unsigned local builds?"
          confirmLabel="Install unsigned builds"
          onConfirm={() => void install()}
          onCancel={() => setConfirmOpen(false)}
        >
          <p>
            These local files skip release signature verification. Firmware flashing still preserves your calibration
            region, and bridge packages still use automatic rollback if they fail to reconnect.
          </p>
          <ol className="updates-steps">
            {chosen.map((a) => (
              <li key={a.component}>
                {COMPONENT_LABELS[a.component]}: <code>{a.name}</code>
              </li>
            ))}
          </ol>
        </ConfirmDialog>
      )}
    </div>
  );
};

/** The Updates page: Keyglass itself, and the keyboard's software. */
export const UpdatesPage: React.FC<{
  updates: UpdatesClient;
  hostFlow: HostUpdateFlow;
  /** Shows whether the background service runs and starts at login. */
  service?: DaemonServiceClient;
  /** Native folder picker for the Local builds developer source. */
  pickFolder?: (title?: string) => Promise<string | null>;
  /** Called with every status the page loads, so the navigation badge stays current. */
  onStatus?: (s: UpdatesStatus) => void;
  /** Called after a keyboard job succeeds, so the rest of Studio reloads the keyboard's config. */
  onKeyboardChanged?: () => void;
  /** Poll intervals for a running check and a running job. */
  pollMs?: number;
  jobPollMs?: number;
}> = ({ updates, hostFlow, service, pickFolder, onStatus, onKeyboardChanged, pollMs = 1500, jobPollMs = 1000 }) => {
  const [status, setStatus] = useState<UpdatesStatus | null>(null);
  const [localDirInput, setLocalDirInput] = useState("");
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
    setLocalDirInput(s.localDir ?? "");
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

  // When the daemon knows of a Keyglass update, ask the updater too so it can be installed.
  const hostAvailable = status?.host.state === "updateAvailable";
  const channel = status?.channel;
  useEffect(() => {
    if (hostAvailable && channel && channel !== "local" && hostFlow.state.phase === "idle") {
      void hostFlow.check(channel);
    }
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
    if (status && status.channel !== "local") void hostFlow.check(status.channel);
  };

  const saveSettings = async (patch: { channel?: UpdateChannel; autoCheck?: boolean; localDir?: string | null }) => {
    setActionError(null);
    try {
      const s = await updates.putUpdateSettings(patch);
      accept(s);
      if (patch.channel && s.channel !== "local") void hostFlow.check(s.channel);
    } catch (e) {
      setActionError(toDaemonError(e));
    }
  };

  const browseLocalDir = async () => {
    if (!pickFolder) return;
    const picked = await pickFolder("Choose local Keyglass build folder");
    if (picked) {
      setLocalDirInput(picked);
      await saveSettings({ localDir: picked });
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
              <option value="local">Local builds (developer folder — unsigned)</option>
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
        {status.channel === "local" && (
          <div className="field">
            <span>Local build folder</span>
            <div className="button-row">
              <input
                type="text"
                aria-label="Local build folder"
                value={localDirInput}
                placeholder="Path to Keyglass checkout or build folder…"
                onChange={(e) => setLocalDirInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void saveSettings({ localDir: localDirInput });
                }}
              />
              {pickFolder && (
                <button type="button" className="btn-secondary" onClick={() => void browseLocalDir()}>
                  Browse…
                </button>
              )}
              <button
                type="button"
                className="btn-secondary"
                onClick={() => void saveSettings({ localDir: localDirInput })}
              >
                Scan
              </button>
            </div>
            <p className="muted">
              Scans this folder (and standard <code>build/</code> subdirectories) for local <code>.uf2</code> and{" "}
              <code>.ipk</code> files. Local builds skip release signature checks.
            </p>
          </div>
        )}
        {status.channel !== "local" && sourceNote && <p className="muted">{sourceNote}</p>}
        {status.error && (
          <ErrorNotice title="The last check failed" code={status.error.code} message={status.error.message} />
        )}
        {status.checking && checkPollError && (
          <DaemonErrorNotice title="Couldn't follow the running check. Trying again…" error={checkPollError.error} />
        )}
        {actionError && <DaemonErrorNotice error={actionError} />}
      </div>

      <HostUpdateCard flow={hostFlow} host={status.host} onOpenUrl={openUrl} />
      {status.channel === "local" && status.localBuilds?.host && (
        <section className="card" aria-label="Local desktop installer">
          <div className="toolbar-between">
            <div>
              <strong>Local Studio installer found</strong>
              <div className="muted">
                <code>{status.localBuilds.host.path}</code> · {formatSizeKb(status.localBuilds.host.sizeBytes)} ·{" "}
                {formatTime(status.localBuilds.host.modifiedAt)}
              </div>
            </div>
            <span className="pill pill-warn">Local (unsigned)</span>
          </div>
        </section>
      )}
      {service && <BackgroundServiceCard service={service} />}

      <section className="card" aria-label="Keyboard">
        <h3>Keyboard</h3>
        {(jobRunning || finishedJob) && (
          <>
            <JobProgress job={jobRunning ? job : finishedJob} unreachable={unreachable} />
            {finishedJob?.state === "succeeded" && finishedJob.kind === "convert" && (
              <>
                <p>The keyboard runs Keyglass, and your settings were imported.</p>
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
        {kb.mode === "openflux" && status.channel !== "local" && (
          <KeyboardUpdates
            updates={updates}
            keyboard={kb}
            jobRunning={jobRunning}
            onJobStarted={startJob}
            onPlanChanged={() => void refresh()}
          />
        )}
        {status.channel === "local" && (
          <LocalKeyboardUpdates
            updates={updates}
            localBuilds={status.localBuilds}
            connected={kb.connected}
            jobRunning={jobRunning}
            onJobStarted={startJob}
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
