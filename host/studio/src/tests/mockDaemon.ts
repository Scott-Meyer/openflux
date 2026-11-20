import { vi } from "vitest";
import { DaemonError } from "../api/daemonError";
import type { DaemonServiceClient, StartReport } from "../api/daemonService";
import type { HostUpdater } from "../api/hostUpdate";
import type { StudioServices } from "../api/services";
import type { Registrations, ThirdPartyClient, ThirdPartySettings } from "../api/thirdParty";
import type {
  ConversionStatus,
  Job,
  JobKind,
  KeyboardStatus,
  LifecycleReply,
  UpdatesClient,
  UpdatesStatus,
} from "../api/updates";

export function refusal(code: string, message: string, status = 409, recovery: string | null = null): DaemonError {
  return new DaemonError({ kind: "daemon", status, code, message, recovery });
}

export const OPENFLUX_KEYBOARD: KeyboardStatus = {
  connected: true,
  mode: "openflux",
  components: [
    {
      component: "firmware",
      installed: "1.0.0",
      kind: "openflux",
      latest: "1.0.0",
      state: "upToDate",
      required: false,
      reason: null,
      notesUrl: null,
    },
    {
      component: "bridge",
      installed: "1.0.0",
      kind: "openflux",
      latest: "1.0.1",
      state: "updateAvailable",
      required: false,
      reason: null,
      notesUrl: "https://example.test/bridge-1.0.1",
    },
    {
      component: "display",
      installed: "1.0.0",
      kind: "openflux",
      latest: "1.0.1",
      state: "updateAvailable",
      required: true,
      reason: null,
      notesUrl: null,
    },
  ],
  plan: {
    steps: [
      { component: "bridge", from: "1.0.0", to: "1.0.1" },
      { component: "display", from: "1.0.0", to: "1.0.1" },
    ],
    reason: null,
  },
  conversion: null,
};

export const FACTORY_KEYBOARD: KeyboardStatus = {
  connected: true,
  mode: "needsConversion",
  components: [
    {
      component: "firmware",
      installed: "1.4.2",
      kind: "factory",
      latest: null,
      state: "conversionRequired",
      reason: null,
      notesUrl: null,
    },
    {
      component: "bridge",
      installed: "1.4.28",
      kind: "factory",
      latest: null,
      state: "conversionRequired",
      reason: null,
      notesUrl: null,
    },
    {
      component: "display",
      installed: "1.4.12",
      kind: "factory",
      latest: null,
      state: "conversionRequired",
      reason: null,
      notesUrl: null,
    },
  ],
  plan: null,
  conversion: {
    found: [],
    install: [
      { component: "firmware", version: "1.0.0" },
      { component: "bridge", version: "1.0.0" },
      { component: "display", version: "1.0.0" },
    ],
  },
};

/**
 * A daemon with the update, job and lifecycle behavior of the shared API: one job at a time,
 * and a stop that refuses new keyboard work at once and exits when the current work is done.
 */
export class MockDaemon {
  status: UpdatesStatus;
  conversion: ConversionStatus = {
    canConvert: true,
    canRestore: false,
    reason: "Factory software isn't available for this keyboard.",
    job: null,
  };
  jobs = new Map<string, Job>();
  /** Keyboard work that isn't a job (a save, a flash), by activity name. */
  leases = new Set<string>();
  stopRequested = false;
  exited = false;
  events: string[] = [];
  /** Runs inside `lifecycle("stop")` before the daemon decides, to stage races. */
  beforeStop: (() => void) | null = null;
  /** Makes the next check end after this many status reads. */
  checkReads = 0;
  restoreError: DaemonError | null = null;
  private nextId = 1;

  constructor(keyboard: KeyboardStatus = OPENFLUX_KEYBOARD) {
    this.status = {
      checkedAt: "2026-01-01T10:00:00Z",
      checking: false,
      channel: "stable",
      autoCheck: true,
      indexSource: "online",
      error: null,
      host: { installed: "1.0.0", latest: "1.0.0", state: "upToDate", reason: null, notesUrl: null },
      keyboard: structuredClone(keyboard),
      job: null,
    };
  }

  private busyNames(): string[] {
    const running = [...this.jobs.values()].filter((j) => j.state === "running").map((j) => j.kind as string);
    return [...running, ...this.leases];
  }

  private assertAlive() {
    if (this.exited) throw new DaemonError({ kind: "notRunning", message: "connection refused" });
  }

  /** Starts keyboard work; refused once a stop was requested. */
  beginWork(activity: string) {
    this.assertAlive();
    if (this.stopRequested) {
      this.events.push(`refused:${activity}`);
      throw refusal("busy", "OpenFlux is stopping");
    }
    this.leases.add(activity);
    this.events.push(`begin:${activity}`);
  }

  endWork(activity: string) {
    this.leases.delete(activity);
    this.events.push(`end:${activity}`);
    this.maybeExit();
  }

  startJob(kind: JobKind, label: string, count = 2): Job {
    this.assertAlive();
    if (this.stopRequested) {
      this.events.push(`refused:${kind}`);
      throw refusal("busy", "OpenFlux is stopping");
    }
    if (this.busyNames().length > 0) throw refusal("busy", "Another job is running");
    const job: Job = {
      id: `job-${this.nextId++}`,
      kind,
      state: "running",
      step: { index: 1, count, label },
      progress: 0,
      log: [],
      error: null,
      startedAt: "2026-01-01T10:01:00Z",
      finishedAt: null,
    };
    this.jobs.set(job.id, job);
    this.status.job = job;
    this.events.push(`job:${job.id}:${kind}`);
    return job;
  }

  progress(id: string, patch: Partial<Job>) {
    const j = this.jobs.get(id) as Job;
    Object.assign(j, patch);
  }

  finish(id: string, state: "succeeded" | "failed", error: Job["error"] = null) {
    const j = this.jobs.get(id) as Job;
    j.state = state;
    j.error = error;
    j.progress = state === "succeeded" ? 1 : j.progress;
    j.finishedAt = "2026-01-01T10:05:00Z";
    this.status.job = null;
    this.events.push(`finished:${id}:${state}`);
    this.maybeExit();
  }

  private maybeExit() {
    if (this.stopRequested && this.busyNames().length === 0 && !this.exited) {
      this.exited = true;
      this.events.push("exited");
    }
  }

  start() {
    this.exited = false;
    this.stopRequested = false;
    this.events.push("started");
  }

  updatesClient(): UpdatesClient {
    return {
      getUpdates: vi.fn(async () => {
        this.assertAlive();
        if (this.status.checking && --this.checkReads <= 0) {
          this.status.checking = false;
          this.status.checkedAt = "2026-01-02T09:30:00Z";
        }
        return structuredClone(this.status);
      }),
      checkUpdates: vi.fn(async () => {
        this.assertAlive();
        this.status.checking = true;
        this.checkReads = this.checkReads || 1;
        return structuredClone(this.status);
      }),
      putUpdateSettings: vi.fn(async (s) => {
        this.assertAlive();
        Object.assign(this.status, s);
        return structuredClone(this.status);
      }),
      planUpdates: vi.fn(async (components) => {
        this.assertAlive();
        const full = this.status.keyboard.plan;
        if (!full) return { plan: null };
        if (!components) return { plan: structuredClone(full) };
        const wanted = new Set(components);
        return {
          plan: {
            steps: full.steps.filter((s) => wanted.has(s.component)),
            reason: full.reason,
          },
        };
      }),
      applyUpdates: vi.fn(async (components, expectedSteps) => {
        if (this.status.keyboard.mode === "needsConversion") {
          throw refusal("conversionRequired", "The keyboard needs conversion first");
        }
        if (!this.status.keyboard.plan) throw refusal("noPlan", "There is nothing to install");
        const first =
          (expectedSteps && expectedSteps[0]) || this.status.keyboard.plan.steps[0];
        this.events.push(`apply:${components ? components.join(",") : "plan"}`);
        const j = this.startJob("update", `Installing ${first.component} ${first.to}`);
        return { jobId: j.id };
      }),
      getConversion: vi.fn(async () => {
        this.assertAlive();
        return structuredClone(this.conversion);
      }),
      startConversion: vi.fn(async (step) => {
        if (step === "restore" && this.restoreError) throw this.restoreError;
        const j = this.startJob(
          step,
          step === "convert" ? "Installing firmware 1.0.0" : "Installing factory software",
          4,
        );
        return { jobId: j.id };
      }),
      getJob: vi.fn(async (id) => {
        this.assertAlive();
        const j = this.jobs.get(id);
        if (!j) throw refusal("notFound", "No such job", 404);
        return structuredClone(j);
      }),
      lifecycle: vi.fn(async (action): Promise<LifecycleReply> => {
        this.events.push(`lifecycle:${action}`);
        if (this.exited) return { outcome: "not-running" };
        if (action === "stop") {
          this.beforeStop?.();
          this.stopRequested = true;
          const busy = this.busyNames();
          if (busy.length > 0) return { outcome: "deferred", exit: "stop", busy };
          this.maybeExit();
          return { outcome: "exiting", exit: "stop" };
        }
        return { outcome: "running", busy: this.busyNames() };
      }),
      openUrl: vi.fn(async () => {}),
    };
  }
}

export function mockHostUpdater(daemon: MockDaemon | null, overrides: Partial<HostUpdater> = {}): HostUpdater {
  return {
    support: vi.fn(async () => ({ mode: "inApp" as const, reason: null, installerExitsApp: false })),
    check: vi.fn(async () => ({ available: true, currentVersion: "1.0.0", version: "1.1.0", notes: "Fixes" })),
    download: vi.fn(async (onProgress) => {
      onProgress({ downloaded: 512, total: 1024 });
      onProgress({ downloaded: 1024, total: 1024 });
    }),
    install: vi.fn(async () => {
      daemon?.events.push(daemon.exited ? "install" : "install-while-daemon-running");
    }),
    discard: vi.fn(async () => {}),
    relaunch: vi.fn(async () => {
      daemon?.events.push("relaunch");
    }),
    ...overrides,
  };
}

export function mockDaemonService(daemon: MockDaemon | null, overrides: Partial<DaemonServiceClient> = {}) {
  const inspection = {
    platform: "macos",
    manager: "launchAgent" as const,
    registration: "enabled" as const,
    daemonBinary: "/Applications/OpenFlux Studio.app/Contents/MacOS/openfluxd",
    detail: null,
  };
  const report: StartReport = {
    before: inspection,
    steps: [{ step: "kickstartLaunchAgent", ok: true, detail: null }],
    after: inspection,
  };
  const client: DaemonServiceClient = {
    status: vi.fn(async () => ({
      reachable: !(daemon?.exited ?? false),
      unreachableReason: null,
      notRunning: daemon?.exited ?? false,
      service: inspection,
    })),
    start: vi.fn(async (mode) => {
      daemon?.events.push(`start:${mode}`);
      daemon?.start();
      return report;
    }),
    openLoginItems: vi.fn(async () => {}),
    ...overrides,
  };
  return client;
}

export class MockThirdParty {
  regs: Registrations = {
    pending: [{ id: "abc123", app: "Editor Plugin", requestedAt: Math.floor(Date.now() / 1000) - 5 }],
    approved: [{ app: "Status Helper", tokens: 1 }],
    denied: [{ app: "Old Tool" }],
  };
  settings: ThirdPartySettings = {
    apiActive: true,
    apiNetworkAccess: false,
    saveToKeyboard: true,
    import: true,
    profileChange: true,
    shortcutChange: true,
    mosaicChange: true,
    domain: null,
  };
  discoveryFailures: string[] = [];

  client(): ThirdPartyClient {
    return {
      getRegistrations: vi.fn(async () => structuredClone(this.regs)),
      decide: vi.fn(async (id, decision) => {
        const p = this.regs.pending.find((r) => r.id === id);
        if (!p) throw refusal("notFound", "No pending registration with that id", 404);
        this.regs.pending = this.regs.pending.filter((r) => r.id !== id);
        if (decision === "approve") this.regs.approved.push({ app: p.app, tokens: 1 });
        else this.regs.denied.push({ app: p.app });
        return { message: `${p.app} ${decision === "approve" ? "approved" : "denied"}` };
      }),
      revoke: vi.fn(async (app, deny) => {
        this.regs.approved = this.regs.approved.filter((a) => a.app !== app);
        if (deny) this.regs.denied.push({ app });
        return { message: `Revoked 1 token(s) for ${app}`, revoked: 1 };
      }),
      forgetDenied: vi.fn(async (app) => {
        this.regs.denied = this.regs.denied.filter((d) => d.app !== app);
        return { message: `${app} may register again` };
      }),
      getSettings: vi.fn(async () => structuredClone(this.settings)),
      putSettings: vi.fn(async (patch) => {
        Object.assign(this.settings, patch);
        return { settings: structuredClone(this.settings), discoveryWriteFailures: this.discoveryFailures };
      }),
    };
  }
}

export function mockServices(daemon: MockDaemon, overrides: Partial<StudioServices> = {}): StudioServices {
  return {
    updates: daemon.updatesClient(),
    thirdParty: new MockThirdParty().client(),
    daemonService: mockDaemonService(daemon),
    hostUpdater: mockHostUpdater(daemon),
    navigation: { initialPage: async () => null, subscribe: () => () => {} },
    ...overrides,
  };
}
