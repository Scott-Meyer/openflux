import { invoke } from "@tauri-apps/api/core";
import { toDaemonError } from "./daemonError";

export interface PendingRegistration {
  id: string;
  app: string;
  requestedAt: number;
}

export interface Registrations {
  pending: PendingRegistration[];
  approved: { app: string; tokens: number }[];
  denied: { app: string }[];
}

export interface ThirdPartySettings {
  apiActive: boolean;
  apiNetworkAccess: boolean;
  saveToKeyboard: boolean;
  import: boolean;
  profileChange: boolean;
  shortcutChange: boolean;
  mosaicChange: boolean;
  domain: unknown;
}

export interface SettingsReply {
  settings: ThirdPartySettings;
  discoveryWriteFailures: string[];
}

/** The daemon's first-party routes for managing third-party apps. */
export interface ThirdPartyClient {
  getRegistrations(): Promise<Registrations>;
  decide(id: string, decision: "approve" | "deny"): Promise<{ message: string }>;
  revoke(app: string, deny: boolean): Promise<{ message: string; revoked: number }>;
  forgetDenied(app: string): Promise<{ message: string }>;
  getSettings(): Promise<ThirdPartySettings>;
  putSettings(patch: Partial<ThirdPartySettings>): Promise<SettingsReply>;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toDaemonError(e);
  }
}

export function createThirdPartyClient(): ThirdPartyClient {
  return {
    getRegistrations: () => call("get_third_party_registrations"),
    decide: (id, decision) => call("decide_third_party_registration", { id, decision }),
    revoke: (app, deny) => call("revoke_third_party_app", { app, deny }),
    forgetDenied: (app) => call("forget_third_party_denied", { app }),
    getSettings: () => call("get_third_party_settings"),
    putSettings: (patch) => call("put_third_party_settings", { patch }),
  };
}
