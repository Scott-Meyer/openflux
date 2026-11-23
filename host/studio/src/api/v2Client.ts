import { invoke } from "@tauri-apps/api/core";

export type ModuleCategory = "effect" | "widget" | "background" | "companion" | "host_service";
export type HostRequirement = "none" | "optional" | "required";
export type ModuleTier = "bundled" | "installed" | "local";

export interface SettingField {
  key: string;
  label: string;
  kind: "number" | "boolean" | "string" | "color" | "select" | "file" | "key";
  default?: unknown;
  min?: number;
  max?: number;
  options?: string[];
  description?: string;
  secret?: boolean;
}

export interface ModuleUiSpec {
  top_tab?: boolean;
  top_tab_label?: string | null;
  top_tab_order?: number;
  builtin_view?: string | null;
}

export interface ModuleActionField {
  id: string;
  label: string;
  topic: string;
  payload?: unknown;
  retain?: boolean;
}

export interface TopBarSpec {
  supported_spans: number[];
  default_span: number;
  singleton: boolean;
}

export interface ModuleManifest {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  category: ModuleCategory;
  tags?: string[];
  provides?: string[];
  stages: string[];
  host_requirement: HostRequirement;
  slot_capable: boolean;
  top_bar?: TopBarSpec | null;
  stackable: boolean;
  forked_from?: string | null;
  ui?: ModuleUiSpec;
  entrypoints: {
    script?: string | null;
    shader?: string | null;
    host?: string | null;
  };
  channels: {
    subscribes: string[];
    publishes: string[];
  };
  settings: SettingField[];
  actions?: ModuleActionField[];
}

export interface ModuleInstanceState {
  enabled: boolean;
  slot?: number | null;
  slot_pos?: number | null;
  slot_span?: number | null;
  pin_tab?: boolean | null;
  config: Record<string, unknown>;
}

export interface ModuleOrigin {
  source: string;
  kind: string;
  subdir?: string | null;
  git_ref?: string | null;
  list_id?: string | null;
  installed_version: string;
  installed_at_ms: number;
}

export interface DependencyIssue {
  module_id: string;
  required_id?: string | null;
  required_role?: string | null;
  min_version?: string | null;
  optional: boolean;
  reason: string;
}

export interface V2ModuleRecord {
  id: string;
  manifest: ModuleManifest;
  tier: ModuleTier;
  path: string;
  state: ModuleInstanceState;
  origin?: ModuleOrigin | null;
  dependency_issues?: DependencyIssue[];
  healthy: boolean;
  last_error?: string | null;
}

export interface MediaLibraryFile {
  name: string;
  path: string;
  bytes: number;
}

export interface InspectedFile {
  rel_path: string;
  kind: string;
  size_bytes: number;
  executable: boolean;
  sha256: string;
}

export interface DisclosureReport {
  module_id: string;
  name: string;
  version: string;
  host_requirement: HostRequirement;
  runs_host_process: boolean;
  host_entrypoint?: string | null;
  has_native_binaries: boolean;
  has_lua_script: boolean;
  has_shaders: boolean;
  subscribes: string[];
  publishes: string[];
  total_bytes: number;
  files: InspectedFile[];
  warnings: string[];
  summary_lines: string[];
}

export interface UpdateDisclosureDiff {
  module_id: string;
  old_version: string;
  new_version: string;
  added_files: InspectedFile[];
  removed_files: InspectedFile[];
  modified_files: InspectedFile[];
  executable_or_host_changed: boolean;
  channels_added: string[];
  channels_removed: string[];
  summary_lines: string[];
}

export interface InstallPreview {
  disclosure: DisclosureReport;
  update_diff?: UpdateDisclosureDiff | null;
}

export interface CuratedListSubscription {
  id: string;
  name: string;
  source: string;
  official: boolean;
}

export interface PreviewFrameTimings {
  lua_ms: number;
  physics_ms: number;
  canvas2d_ms: number;
  gpu_submit_ms: number;
  total_frame_ms: number;
  particle_count: number;
  glow_count: number;
  wave_enabled: boolean;
}

export interface PreviewFrameResponse {
  width: number;
  height: number;
  gpu_adapter: string;
  modules_rev: number;
  png_base64: string;
  modules: Array<{ id: string; healthy: boolean; last_error?: string | null }>;
  timings?: PreviewFrameTimings;
}

export interface ModuleLogEntry {
  timestamp_ms: number;
  level: string;
  message: string;
  module_id?: string;
}

async function v2Call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  if (isTauri) {
    return invoke<T>("v2_request", { method, path, body: body ?? null });
  }
  const resp = await fetch(path, {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const data = await resp.json();
  if (!resp.ok) {
    throw new Error(data?.error ?? data?.message ?? `HTTP ${resp.status}`);
  }
  return data as T;
}

export const v2Client = {
  getStatus: () =>
    v2Call<{
      pid: number;
      usb_protocol?: string;
      modules_rev: number;
      latest_bus_seq: number;
      gpu_adapter: string;
    }>("GET", "/v2/status"),
  listModules: () => v2Call<{ modules: V2ModuleRecord[]; modules_rev: number }>("GET", "/v2/modules"),
  getModuleDetail: (id: string) =>
    v2Call<{
      id: string;
      manifest: ModuleManifest;
      disclosure?: DisclosureReport;
      logs: ModuleLogEntry[];
    }>("GET", `/v2/modules/${encodeURIComponent(id)}`),
  getModuleLogs: (id: string) =>
    v2Call<{ id: string; logs: ModuleLogEntry[] }>("GET", `/v2/modules/${encodeURIComponent(id)}/logs`),
  pollBusEvents: (pattern: string, sinceSeq: number, waitMs = 2000) =>
    v2Call<{ events: Array<{ seq: number; topic: string; payload: unknown }>; latest_seq: number }>(
      "GET",
      `/v2/bus/events?pattern=${encodeURIComponent(pattern)}&since_seq=${sinceSeq}&wait_ms=${waitMs}`,
    ),
  updateModuleState: (
    id: string,
    patch: {
      enabled?: boolean;
      slot?: number | null;
      slot_pos?: number | null;
      slot_span?: number | null;
      pin_tab?: boolean | null;
      config?: Record<string, unknown>;
    },
  ) => v2Call("PUT", `/v2/modules/${encodeURIComponent(id)}/state`, patch),
  triggerAction: (id: string, action_id: string) =>
    v2Call("POST", `/v2/modules/${encodeURIComponent(id)}/action`, { action_id }),
  createModule: (body: { id: string; category: ModuleCategory; name?: string; enable?: boolean; slot?: number }) =>
    v2Call("POST", "/v2/modules/create", body),
  forkModule: (body: { source_id: string; new_id: string; name?: string; enable?: boolean }) =>
    v2Call("POST", "/v2/modules/fork", body),
  inspectModule: (source: string) =>
    v2Call<{ preview: InstallPreview }>("POST", "/v2/modules/inspect", { source }),
  installModule: (source: string, accept_disclosure: boolean) =>
    v2Call<{ installed: boolean; requires_acceptance?: boolean; preview: InstallPreview }>(
      "POST",
      "/v2/modules/install",
      { source, accept_disclosure },
    ),
  uninstallModule: (id: string) =>
    v2Call<{ uninstalled: boolean; id: string; removed_tier: string }>(
      "DELETE",
      `/v2/modules/${encodeURIComponent(id)}`,
    ),
  updateModule: (id: string, accept_disclosure: boolean) =>
    v2Call<{ updated: boolean; requires_acceptance?: boolean; preview: InstallPreview }>(
      "POST",
      `/v2/modules/${encodeURIComponent(id)}/update`,
      { accept_disclosure },
    ),
  reloadModule: (id: string) => v2Call("POST", `/v2/modules/${encodeURIComponent(id)}/reload`, {}),
  listCuratedLists: () => v2Call<{ lists: CuratedListSubscription[] }>("GET", "/v2/lists"),
  browseCuratedList: (source: string) =>
    v2Call<{
      list: {
        name: string;
        description: string;
        entries: Array<{ id: string; name: string; version: string; description: string; source: string }>;
      };
    }>("GET", `/v2/lists/browse?source=${encodeURIComponent(source)}`),
  listMediaLibrary: () =>
    v2Call<{ media_dir: string; files: MediaLibraryFile[] }>("GET", "/v2/media/library"),
  addCuratedList: (sub: CuratedListSubscription) => v2Call("POST", "/v2/lists", sub),
  removeCuratedList: (id: string) => v2Call("DELETE", `/v2/lists/${encodeURIComponent(id)}`),
  exportPreset: (name: string, description: string) =>
    v2Call<{ preset: Record<string, unknown> }>("POST", "/v2/preset/export", { name, description }),
  importPreset: (preset: Record<string, unknown>) => v2Call("POST", "/v2/preset/import", preset),
  publishBus: (topic: string, payload: unknown, retain = false) =>
    v2Call("POST", "/v2/bus/pub", { topic, payload, retain }),
  simulateKey: (key_index: number, depth_um = 2400, velocity_um_s = 15000, release = false) =>
    v2Call("POST", "/v2/simulate/key", { key_index, depth_um, velocity_um_s, release }),
  simulateImpulse: (x: number, y: number, radius = 64, force = 1.15) =>
    v2Call("POST", "/v2/simulate/impulse", { x, y, radius, force, kind: "ripple", source: "studio" }),
  simulateBurst: () => v2Call("POST", "/v2/simulate/burst", {}),
  syncToKeyboard: () =>
    v2Call<{ ok: boolean; uploaded_files: string[]; uploaded_bytes: number }>("POST", "/v2/keyboard/sync", {}),
  renderPreviewFrame: (steps = 2, step_ms = 16.667, width = 960, height = 540) =>
    v2Call<PreviewFrameResponse>("POST", "/v2/preview/frame", { steps, step_ms, width, height }),
  vialStatus: () =>
    v2Call<{ connected: boolean; product_name: string; vendor_id: number; product_id: number }>(
      "GET",
      "/v2/vial/status",
    ),
  vialXfer: (data: number[]) =>
    v2Call<{ data: number[] }>("POST", "/v2/vial/xfer", { data }),
  vialSyncActions: () =>
    v2Call<{
      ok: boolean;
      firmware_synced: boolean;
      soc_synced: boolean;
      xz_bytes: number;
      slot_count: number;
      warnings: string[];
    }>("POST", "/v2/vial/sync", {}),
};
