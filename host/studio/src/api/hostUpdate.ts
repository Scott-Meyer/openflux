import { Channel, invoke } from "@tauri-apps/api/core";
import { toDaemonError } from "./daemonError";
import type { UpdateChannel } from "./updates";

export interface HostUpdateSupport {
  /** `inApp`: Studio installs updates. `notify`: Studio links to the release. */
  mode: "inApp" | "notify";
  reason: string | null;
  /** The Windows setup program replaces the running app, so Studio exits when it starts. */
  installerExitsApp: boolean;
}

export interface HostUpdateCheck {
  available: boolean;
  currentVersion: string;
  version?: string;
  notes?: string | null;
  date?: string | null;
}

export interface DownloadProgress {
  downloaded: number;
  total: number | null;
}

/** The Tauri updater, one step per call. */
export interface HostUpdater {
  support(): Promise<HostUpdateSupport>;
  check(channel: UpdateChannel): Promise<HostUpdateCheck>;
  /** Downloads and verifies the update found by the last check. */
  download(onProgress: (p: DownloadProgress) => void): Promise<void>;
  install(): Promise<void>;
  discard(): Promise<void>;
  relaunch(): Promise<void>;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toDaemonError(e);
  }
}

export function createHostUpdater(): HostUpdater {
  return {
    support: () => call("host_update_support"),
    check: (channel) => call("host_update_check", { channel }),
    download: async (onProgress) => {
      const ch = new Channel<DownloadProgress>();
      ch.onmessage = onProgress;
      await call("host_update_download", { onProgress: ch });
    },
    install: () => call("host_update_install"),
    discard: () => call("host_update_discard"),
    relaunch: () => call("relaunch_studio"),
  };
}
