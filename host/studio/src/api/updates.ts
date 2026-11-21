import { invoke } from "@tauri-apps/api/core";
import { toDaemonError } from "./daemonError";

export type UpdateChannel = "stable" | "beta" | "local";
export type KeyboardComponent = "firmware" | "bridge" | "display";

export interface LocalArtifact {
  component: KeyboardComponent;
  name: string;
  path: string;
  version: string | null;
  sizeBytes: number;
  modifiedAt: string;
}

export interface LocalHostInstaller {
  name: string;
  path: string;
  sizeBytes: number;
  modifiedAt: string;
}

export interface LocalScan {
  dir: string;
  exists: boolean;
  error: string | null;
  firmware: LocalArtifact | null;
  bridge: LocalArtifact | null;
  display: LocalArtifact | null;
  host: LocalHostInstaller | null;
}

export interface ApiErrorInfo {
  code: string;
  message: string;
  recovery?: string | null;
}

export interface HostStatus {
  installed: string;
  latest: string | null;
  state: "upToDate" | "updateAvailable" | "blocked";
  reason: string | null;
  notesUrl: string | null;
}

export type ComponentState =
  | "upToDate"
  | "updateAvailable"
  | "needsNewerHost"
  | "newerThanHost"
  | "conversionRequired"
  | "blocked"
  | "incompatible"
  | "unknown";

export interface ComponentStatus {
  component: KeyboardComponent;
  installed: string | null;
  kind: "openflux" | "preRelease" | "factory" | "unknown";
  customBuild?: boolean;
  latest: string | null;
  state: ComponentState;
  required?: boolean;
  reason: string | null;
  notesUrl: string | null;
}

export interface PlanStep {
  component: KeyboardComponent;
  from: string | null;
  to: string;
}

export interface ExpectedPlanStep {
  component: KeyboardComponent;
  to: string;
}

export interface UpdatePlan {
  steps: PlanStep[];
  reason: string | null;
}

export interface KeyboardStatus {
  connected: boolean;
  mode: "openflux" | "needsConversion" | "disconnected";
  components: ComponentStatus[];
  plan: UpdatePlan | null;
  conversion: { found: ComponentStatus[]; install: { component: KeyboardComponent; version: string }[] } | null;
}

export type JobKind = "update" | "convert" | "restore" | "localInstall";

export interface Job {
  id: string;
  kind: JobKind;
  state: "running" | "succeeded" | "failed";
  step: { index: number; count: number; label: string } | null;
  progress: number | null;
  log: string[];
  error: (ApiErrorInfo & { recovery?: string | null }) | null;
  startedAt: string | null;
  finishedAt: string | null;
  /** A conversion's press-refinement step, once it ran. */
  pressRefinement?: {
    installed: boolean;
    /** For installed data: whether the firmware applies it. */
    active?: boolean;
    crc32?: number;
    code?: string;
    message?: string;
  } | null;
}

export interface UpdatesStatus {
  checkedAt: string | null;
  checking: boolean;
  channel: UpdateChannel;
  autoCheck: boolean;
  localDir?: string | null;
  localBuilds?: LocalScan | null;
  indexSource: "online" | "bundled" | "none";
  error: ApiErrorInfo | null;
  host: HostStatus;
  keyboard: KeyboardStatus;
  job: Job | null;
}

export interface ConversionStatus {
  canConvert: boolean;
  canRestore: boolean;
  reason: string | null;
  job: Job | null;
}

/** `convert`: install OpenFlux on the whole keyboard. `restore`: reinstall the factory software. */
export type ConversionStep = "convert" | "restore";

/** `POST /v1/daemon/lifecycle`. `not-running` means no daemon answered. */
export interface LifecycleReply {
  outcome: "running" | "exiting" | "deferred" | "not-running";
  busy?: string[];
  exit?: string;
  instanceId?: string;
}

/** The daemon's update, conversion and job routes. */
export interface UpdatesClient {
  getUpdates(): Promise<UpdatesStatus>;
  checkUpdates(): Promise<UpdatesStatus>;
  putUpdateSettings(settings: {
    channel?: UpdateChannel;
    autoCheck?: boolean;
    localDir?: string | null;
  }): Promise<UpdatesStatus>;
  /** Previews the exact plan for `components` (or the whole keyboard when omitted). */
  planUpdates?(components?: KeyboardComponent[]): Promise<{ plan: UpdatePlan | null }>;
  /** Starts installing keyboard updates. Omitting `components` installs the whole plan. */
  applyUpdates(components?: KeyboardComponent[], expectedSteps?: ExpectedPlanStep[]): Promise<{ jobId: string }>;
  /** Starts installing unsigned local keyboard builds from the configured local folder. */
  installLocalUpdates?(components: KeyboardComponent[]): Promise<{ jobId: string }>;
  getConversion(): Promise<ConversionStatus>;
  startConversion(step: ConversionStep): Promise<{ jobId: string }>;
  getJob(id: string): Promise<Job>;
  lifecycle(action: "status" | "stop" | "restart"): Promise<LifecycleReply>;
  /** Opens an `https` page, such as release notes, in the browser. */
  openUrl(url: string): Promise<void>;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toDaemonError(e);
  }
}

/** A started job's id; the daemon answers 202 with `{"jobId"}` or the job itself. */
function jobIdOf(r: { jobId?: string; id?: string }): { jobId: string } {
  return { jobId: String(r.jobId ?? r.id) };
}

export function createUpdatesClient(): UpdatesClient {
  return {
    getUpdates: () => call("get_updates"),
    checkUpdates: () => call("check_updates"),
    putUpdateSettings: (settings) => call("put_update_settings", { settings }),
    planUpdates: (components) => call("plan_updates", { components: components ?? null }),
    applyUpdates: async (components, expectedSteps) =>
      jobIdOf(
        await call("apply_updates", {
          components: components ?? null,
          expectedSteps: expectedSteps ?? null,
        }),
      ),
    installLocalUpdates: async (components) => jobIdOf(await call("install_local_updates", { components })),
    getConversion: () => call("get_conversion"),
    startConversion: async (step) => jobIdOf(await call("start_conversion", { step })),
    getJob: (id) => call("get_job", { id }),
    lifecycle: (action) => call("daemon_lifecycle", { action }),
    openUrl: (url) => call("open_url", { url }),
  };
}

export const COMPONENT_LABELS: Record<KeyboardComponent, string> = {
  firmware: "Key scanner firmware",
  bridge: "Keyboard bridge",
  display: "Display",
};

/** Whether anything on the Updates page asks for the user's attention. */
export function updatesNeedAttention(s: UpdatesStatus | null): boolean {
  if (!s) return false;
  return (
    s.host.state === "updateAvailable" ||
    s.keyboard.mode === "needsConversion" ||
    s.keyboard.components.some((c) => c.state === "updateAvailable")
  );
}
