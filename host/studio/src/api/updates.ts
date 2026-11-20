import { invoke } from "@tauri-apps/api/core";
import { toDaemonError } from "./daemonError";

export type UpdateChannel = "stable" | "beta";
export type KeyboardComponent = "firmware" | "bridge" | "display";

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

export interface KeyboardStatus {
  connected: boolean;
  mode: "openflux" | "needsConversion" | "disconnected";
  components: ComponentStatus[];
  plan: { steps: PlanStep[]; reason: string | null } | null;
  conversion: { found: ComponentStatus[]; install: { component: KeyboardComponent; version: string }[] } | null;
}

export type JobKind = "update" | "convert" | "restore";

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
  putUpdateSettings(settings: { channel?: UpdateChannel; autoCheck?: boolean }): Promise<UpdatesStatus>;
  /** Starts installing keyboard updates. Omitting `components` installs the whole plan. */
  applyUpdates(components?: KeyboardComponent[]): Promise<{ jobId: string }>;
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
    applyUpdates: async (components) => jobIdOf(await call("apply_updates", { components: components ?? null })),
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
