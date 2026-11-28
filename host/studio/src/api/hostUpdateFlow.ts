import { DaemonError, toDaemonError } from "./daemonError";
import type { StartMode, StartReport } from "./daemonService";
import type { HostUpdateCheck, HostUpdater, HostUpdateSupport } from "./hostUpdate";
import type { Job, LifecycleReply, UpdateChannel } from "./updates";

/**
 * Installing an update of Keyglass itself.
 *
 * The update bundle replaces the daemon too, so the daemon must not be in the middle of work on
 * the keyboard when the files change. The flow:
 * 1. download and verify the update, without involving the daemon;
 * 2. ask the daemon to stop. From that moment it refuses new keyboard work and exits once the
 *    current work finishes; until then Studio shows what it is waiting for and never starts or
 *    re-registers the daemon;
 * 3. install;
 * 4. start the new daemon through the service manager and relaunch Studio.
 * If the install fails, or the user cancels after the stop was requested, the existing daemon is
 * started again and the user is told.
 */
export type HostUpdateState =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "unsupported"; reason: string }
  | { phase: "upToDate"; currentVersion: string }
  | { phase: "available"; update: HostUpdateCheck }
  | { phase: "downloading"; update: HostUpdateCheck; downloaded: number; total: number | null }
  | { phase: "stoppingDaemon"; update: HostUpdateCheck }
  | { phase: "waitingForKeyboard"; update: HostUpdateCheck; busy: string[]; job: Job | null; cancelling: boolean }
  | { phase: "installing"; update: HostUpdateCheck }
  | { phase: "relaunching"; update: HostUpdateCheck }
  | { phase: "cancelled"; update: HostUpdateCheck; daemonRestarted: boolean | null }
  | {
      phase: "failed";
      stage: "check" | "download" | "stop" | "install";
      update: HostUpdateCheck | null;
      error: DaemonError;
      /** Whether the existing daemon was started again; `null` when it was never stopped. */
      daemonRestarted: boolean | null;
    };

export interface HostUpdateDeps {
  updater: HostUpdater;
  lifecycle(action: "status" | "stop"): Promise<LifecycleReply>;
  /** The daemon's running job, for "waiting for … to finish". */
  currentJob(): Promise<Job | null>;
  startDaemon(mode: StartMode): Promise<StartReport>;
  sleep?: (ms: number) => Promise<void>;
  /** Poll interval while waiting for the daemon to exit. */
  pollMs?: number;
  /** How long an idle daemon may take to exit after agreeing to stop. */
  exitTimeoutMs?: number;
}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export class HostUpdateFlow {
  private stateValue: HostUpdateState = { phase: "idle" };
  private listeners = new Set<(s: HostUpdateState) => void>();
  private cancelRequested = false;
  private supportValue: HostUpdateSupport | null = null;
  private checking: Promise<HostUpdateState> | null = null;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly pollMs: number;
  private readonly exitTimeoutMs: number;

  constructor(private readonly deps: HostUpdateDeps) {
    this.sleep = deps.sleep ?? defaultSleep;
    this.pollMs = deps.pollMs ?? 1500;
    this.exitTimeoutMs = deps.exitTimeoutMs ?? 30_000;
  }

  get state(): HostUpdateState {
    return this.stateValue;
  }

  subscribe(fn: (s: HostUpdateState) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private set(s: HostUpdateState) {
    this.stateValue = s;
    for (const l of this.listeners) l(s);
  }

  /** True while the flow is past the point where the user can simply walk away. */
  get busy(): boolean {
    return ["downloading", "stoppingDaemon", "waitingForKeyboard", "installing", "relaunching"].includes(
      this.stateValue.phase,
    );
  }

  async support(): Promise<HostUpdateSupport> {
    if (!this.supportValue) this.supportValue = await this.deps.updater.support();
    return this.supportValue;
  }

  /**
   * Checks `channel` for an update. Checks run one at a time, in the order asked, so the result
   * shown is always the last one the desktop shell found (the update a download would fetch). A
   * check that comes due after an install has begun changes nothing.
   */
  check(channel: UpdateChannel): Promise<HostUpdateState> {
    if (channel === "local") return Promise.resolve(this.stateValue);
    const run = (this.checking ?? Promise.resolve(this.stateValue)).then(() => this.runCheck(channel));
    this.checking = run;
    void run.finally(() => {
      if (this.checking === run) this.checking = null;
    });
    return run;
  }

  private async runCheck(channel: UpdateChannel): Promise<HostUpdateState> {
    if (this.busy) return this.stateValue;
    this.set({ phase: "checking" });
    try {
      const support = await this.support();
      if (support.mode !== "inApp") {
        this.set({ phase: "unsupported", reason: support.reason ?? "" });
        return this.stateValue;
      }
      const r = await this.deps.updater.check(channel);
      this.set(
        r.available ? { phase: "available", update: r } : { phase: "upToDate", currentVersion: r.currentVersion },
      );
    } catch (e) {
      this.set({ phase: "failed", stage: "check", update: null, error: toDaemonError(e), daemonRestarted: null });
    }
    return this.stateValue;
  }

  /**
   * Cancels the update. Before the stop request nothing has changed. After it, the daemon still
   * exits once its work is done (a stop can't be withdrawn), so the flow waits for that and then
   * starts the existing daemon again instead of installing.
   */
  cancel() {
    const s = this.stateValue;
    if (s.phase === "available" || s.phase === "failed" || s.phase === "upToDate") {
      this.set({ phase: "idle" });
      return;
    }
    if (!this.busy || s.phase === "installing" || s.phase === "relaunching") return;
    this.cancelRequested = true;
    if (s.phase === "waitingForKeyboard") this.set({ ...s, cancelling: true });
  }

  async install(): Promise<HostUpdateState> {
    const s = this.stateValue;
    if (s.phase !== "available" && !(s.phase === "failed" && s.update)) return this.stateValue;
    const update = s.update as HostUpdateCheck;
    this.cancelRequested = false;

    // 1. Download and verify. The daemon is not involved yet.
    this.set({ phase: "downloading", update, downloaded: 0, total: null });
    try {
      await this.deps.updater.download((p) => {
        if (this.stateValue.phase === "downloading") this.set({ ...this.stateValue, ...p });
      });
    } catch (e) {
      this.set({ phase: "failed", stage: "download", update, error: toDaemonError(e), daemonRestarted: null });
      return this.stateValue;
    }
    if (this.cancelRequested) {
      await this.deps.updater.discard().catch(() => {});
      this.set({ phase: "cancelled", update, daemonRestarted: null });
      return this.stateValue;
    }

    // 2. Stop the daemon. Once it agrees it takes no new keyboard work.
    this.set({ phase: "stoppingDaemon", update });
    try {
      await this.deps.lifecycle("stop");
    } catch (e) {
      const err = toDaemonError(e);
      // A refusal means the daemon is running and said no. Any other failure may still have
      // been a stop; waiting for a confirmed exit covers both cases.
      if (err.kind === "daemon") {
        await this.deps.updater.discard().catch(() => {});
        this.set({ phase: "failed", stage: "stop", update, error: err, daemonRestarted: null });
        return this.stateValue;
      }
    }
    const exit = await this.waitForExit(update);
    if (exit !== true) {
      await this.deps.updater.discard().catch(() => {});
      this.set({ phase: "failed", stage: "stop", update, error: exit, daemonRestarted: null });
      return this.stateValue;
    }
    if (this.cancelRequested) {
      await this.deps.updater.discard().catch(() => {});
      const restarted = await this.restartExisting();
      this.set({ phase: "cancelled", update, daemonRestarted: restarted });
      return this.stateValue;
    }

    // 3. Install. On Windows the setup program takes over here and Studio exits.
    this.set({ phase: "installing", update });
    try {
      await this.deps.updater.install();
    } catch (e) {
      const restarted = await this.restartExisting();
      this.set({ phase: "failed", stage: "install", update, error: toDaemonError(e), daemonRestarted: restarted });
      return this.stateValue;
    }

    // 4. Start the new daemon and relaunch. The relaunched Studio starts the daemon itself if
    // the service manager can't.
    this.set({ phase: "relaunching", update });
    await this.deps.startDaemon("serviceOnly").catch(() => undefined);
    await this.deps.updater.relaunch();
    return this.stateValue;
  }

  /**
   * Polls until the daemon is confirmed gone: only a `not-running` answer counts, which the
   * desktop shell gives on positive evidence (no discovery file or a refused port, and a free
   * instance lock). Errors and an idle daemon that doesn't exit block the install once
   * `exitTimeoutMs` passes; keyboard work is waited for as long as it runs.
   */
  private async waitForExit(update: HostUpdateCheck): Promise<true | DaemonError> {
    let unconfirmed = 0;
    let lastError: DaemonError | null = null;
    for (;;) {
      let reply: LifecycleReply | null = null;
      try {
        reply = await this.deps.lifecycle("status");
      } catch (e) {
        lastError = toDaemonError(e);
      }
      if (reply?.outcome === "not-running") return true;
      const busy = reply?.busy ?? [];
      if (busy.length > 0) {
        unconfirmed = 0;
        lastError = null;
        const job = await this.deps.currentJob().catch(() => null);
        this.set({ phase: "waitingForKeyboard", update, busy, job, cancelling: this.cancelRequested });
      } else {
        unconfirmed += this.pollMs;
        if (unconfirmed > this.exitTimeoutMs) {
          return (
            lastError ??
            new DaemonError({
              kind: "local",
              message: "The background service agreed to stop but is still running.",
              recovery: "Quit Keyglass Studio and open it again, then retry the update.",
            })
          );
        }
      }
      await this.sleep(this.pollMs);
    }
  }

  /** Starts the daemon that was stopped for the update and reports whether it answers. */
  private async restartExisting(): Promise<boolean> {
    try {
      await this.deps.startDaemon("auto");
    } catch {
      return false;
    }
    for (let i = 0; i < 20; i++) {
      try {
        const r = await this.deps.lifecycle("status");
        if (r.outcome !== "not-running") return true;
      } catch {
        /* still starting */
      }
      await this.sleep(500);
    }
    return false;
  }
}
