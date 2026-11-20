import { invoke } from "@tauri-apps/api/core";
import { toDaemonError } from "./daemonError";

export type ServiceManager = "launchAgent" | "runKey" | "systemdUser" | "none";
export type ServiceRegistration = "enabled" | "requiresApproval" | "notRegistered" | "notFound" | "unknown";
export type ServiceStep =
  | "registerLaunchAgent"
  | "kickstartLaunchAgent"
  | "writeRunEntry"
  | "startSystemdUnit"
  | "startBundledDaemon";
/**
 * `auto`: use the service manager, or start the bundled daemon directly if that fails.
 * `serviceOnly`: the service manager only (after a host update). `direct`: this session only.
 */
export type StartMode = "auto" | "serviceOnly" | "direct";

export interface ServiceInspection {
  platform: "macos" | "windows" | "linux" | "other" | string;
  manager: ServiceManager;
  registration: ServiceRegistration;
  daemonBinary: string | null;
  detail: string | null;
}

export interface ServiceStatus {
  reachable: boolean;
  unreachableReason: string | null;
  notRunning: boolean;
  service: ServiceInspection;
}

export interface StartReport {
  before: ServiceInspection;
  steps: { step: ServiceStep; ok: boolean; detail: string | null }[];
  after: ServiceInspection;
}

/** Finding, registering and starting the daemon on this computer. */
export interface DaemonServiceClient {
  status(): Promise<ServiceStatus>;
  start(mode?: StartMode): Promise<StartReport>;
  openLoginItems(): Promise<void>;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toDaemonError(e);
  }
}

export function createDaemonServiceClient(): DaemonServiceClient {
  return {
    status: () => call("daemon_service_status"),
    start: (mode) => call("daemon_service_start", { mode: mode ?? "auto" }),
    openLoginItems: () => call("open_login_items_settings"),
  };
}

export const STEP_LABELS: Record<ServiceStep, string> = {
  registerLaunchAgent: "Registering the background service as a login item",
  kickstartLaunchAgent: "Starting the background service",
  writeRunEntry: "Adding the background service to sign-in",
  startSystemdUnit: "Starting the background service",
  startBundledDaemon: "Starting the background service",
};
