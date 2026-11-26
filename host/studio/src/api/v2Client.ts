import { invoke } from "@tauri-apps/api/core";

export type ModuleCategory = "effect" | "widget" | "background" | "companion" | "host_service";
export type HostRequirement = "none" | "optional" | "required";
export type ModuleTier = "bundled" | "installed" | "local";
export type ModuleMaturity = "stable" | "beta" | "alpha" | "deprecated" | "abandoned";

export type ScalarSettingKind = "number" | "boolean" | "string" | "color" | "select" | "file" | "key";
export type SettingKind = ScalarSettingKind | "list";

export interface SettingField {
  key: string;
  label: string;
  kind: SettingKind;
  default?: unknown;
  min?: number;
  max?: number;
  options?: string[];
  description?: string;
  secret?: boolean;
  /**
   * For `kind: "list"`: the schema of one row. The stored value is a JSON array of row objects keyed
   * by these fields' keys. Row fields are always scalar (never lists). Absent for scalar settings.
   */
  fields?: SettingField[];
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
  license?: string | null;
  homepage?: string | null;
  repository?: string | null;
  issues_url?: string | null;
  support_url?: string | null;
  discord_url?: string | null;
  changelog?: string | null;
  changelog_url?: string | null;
  maturity?: ModuleMaturity;
  deprecated?: boolean;
  deprecation_message?: string | null;
  moved_to?: string | null;
  replaced_by?: string | null;
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
  /** The module declares more bindable actions at runtime on `system/actions/declared/<id>`. */
  dynamic_actions?: boolean;
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

export type HostProcessStatus =
  | { state: "not_required" }
  | { state: "running"; pid: number }
  | { state: "failed"; error: string }
  | { state: "exited"; code?: number | null; last_stderr?: string | null };

export type AttachmentKind = "clear" | "dial" | "triple_dial" | "slider" | "macro_4" | "blank";

export interface HardwareAttachmentBay {
  pos: number;
  span: number;
  kind: AttachmentKind;
  opaque?: boolean;
  label?: string;
}

export interface KeyboardAttachmentsState {
  bays: HardwareAttachmentBay[];
  clear_rects: Array<{ pos: number; span: number; x: number; y: number; w: number; h: number }>;
  blocked_rects: Array<{ pos: number; span: number; kind: string; label: string; x: number; y: number; w: number; h: number }>;
}

export interface ContextTopBarSlot {
  module_id: string;
  pos: number;
  span: number;
}

export interface ContextRule {
  id: string;
  name: string;
  enabled: boolean;
  match_apps: string[];
  qmk_layer?: number | null;
  top_bar_slots?: ContextTopBarSlot[] | null;
}

export interface HostIdentity {
  host_id: string;
  host_name: string;
  os: string;
}

export interface ProfileEntry {
  id: string;
  name: string;
  parent?: string | null;
  enabled: boolean;
  implicit?: boolean;
  match_apps: string[];
  match_hosts: string[];
  match_os: string[];
  tags: string[];
  qmk_layer?: number | null;
  top_bar_slots?: ContextTopBarSlot[] | null;
  module_overrides: Record<string, Record<string, unknown>>;
}

export interface ProfileStackLayer {
  id: string;
  name: string;
  scope: "base" | "os" | "host" | "app" | string;
  implicit: boolean;
  pinned: boolean;
  qmk_layer?: number | null;
  override_count: number;
  summary: string;
}

export interface ProfileFieldSource {
  profile_id: string;
  profile_name: string;
  scope: string;
}

export interface ActiveProfileState {
  profile_id: string;
  profile_name: string;
  chain: string[];
  stack?: ProfileStackLayer[];
  field_sources?: Record<string, ProfileFieldSource>;
  source: string;
  manual_override: boolean;
  host?: HostIdentity;
  app_name?: string;
  tags: string[];
  qmk_layer?: number | null;
  top_bar_slots?: ContextTopBarSlot[] | null;
  module_overrides: Record<string, Record<string, unknown>>;
}

export interface StreamDeckPluginAction {
  uuid: string;
  name: string;
  tooltip?: string;
  icon?: string | null;
}

export interface StreamDeckPluginInfo {
  id: string;
  name: string;
  author?: string;
  version?: string;
  description?: string;
  code_path?: string | null;
  property_inspector?: string | null;
  path: string;
  actions: StreamDeckPluginAction[];
}

export interface StreamDeckButton {
  id: string;
  title: string;
  plugin_id: string;
  action_uuid: string;
  icon_media?: string | null;
  color?: string | null;
  settings: Record<string, unknown>;
}

export interface RemovedModuleRecord {
  id: string;
  name: string;
  version: string;
  description: string;
  source: string;
  list_id?: string | null;
  removed_at_ms: number;
}

export interface ModuleUpdateInfo {
  id: string;
  name: string;
  current_version: string;
  available_version: string;
  has_update: boolean;
  changelog?: string | null;
  source: string;
  list_name?: string | null;
  runs_host_process?: boolean;
  host_changed?: boolean;
  maturity?: ModuleMaturity;
  deprecated?: boolean;
  deprecation_message?: string | null;
  moved_to?: string | null;
  replaced_by?: string | null;
}

export interface V2ModuleRecord {
  id: string;
  manifest: ModuleManifest;
  tier: ModuleTier;
  path: string;
  state: ModuleInstanceState;
  origin?: ModuleOrigin | null;
  dependency_issues?: DependencyIssue[];
  host_status?: HostProcessStatus;
  healthy: boolean;
  last_error?: string | null;
}

export interface BindingLayout {
  schema: 1;
  faces: Array<{ key: string; mods: number; name: string; icon?: string }>;
  flavor?: string;
  skipped?: number;
}

interface BindingLayoutResponse {
  layout: BindingLayout | null;
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
  background_media_name?: string | null;
  bg_dimming?: number;
  top_fade?: number;
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
  listModules: () =>
    v2Call<{ modules: V2ModuleRecord[]; removed_sources?: RemovedModuleRecord[]; modules_rev: number }>(
      "GET",
      "/v2/modules",
    ),
  checkModuleUpdates: () =>
    v2Call<{ updates: ModuleUpdateInfo[]; available_count: number }>("GET", "/v2/modules/updates"),
  forgetRemovedSource: (id: string) =>
    v2Call<{ ok: boolean }>("DELETE", `/v2/modules/removed/${encodeURIComponent(id)}`),
  getModuleDetail: (id: string) =>
    v2Call<{
      id: string;
      manifest: ModuleManifest;
      host_status?: HostProcessStatus;
      disclosure?: DisclosureReport;
      readme?: string | null;
      logs: ModuleLogEntry[];
    }>("GET", `/v2/modules/${encodeURIComponent(id)}`),
  getModuleLogs: (id: string) =>
    v2Call<{ id: string; logs: ModuleLogEntry[] }>("GET", `/v2/modules/${encodeURIComponent(id)}/logs`),
  getBindingLayout: (id: string) =>
    v2Call<BindingLayoutResponse>("GET", `/v2/modules/${encodeURIComponent(id)}/binding-layout`),
  importBindingLayout: (id: string, contents: string) =>
    v2Call<BindingLayoutResponse>("POST", `/v2/modules/${encodeURIComponent(id)}/binding-layout`, { contents }),
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
    v2Call<{ preview: InstallPreview; inspect_token: string }>("POST", "/v2/modules/inspect", { source }),
  installModule: (source: string, accept_disclosure: boolean, inspect_token?: string) =>
    v2Call<{ installed: boolean; requires_acceptance?: boolean; preview: InstallPreview }>(
      "POST",
      "/v2/modules/install",
      { source, accept_disclosure, ...(inspect_token ? { inspect_token } : {}) },
    ),
  migrateModule: (oldId: string, source: string, accept_disclosure: boolean, inspect_token?: string) =>
    v2Call<{ migrated: boolean; old_id: string; new_id: string; preview: InstallPreview }>(
      "POST",
      `/v2/modules/${encodeURIComponent(oldId)}/migrate`,
      { source, accept_disclosure, ...(inspect_token ? { inspect_token } : {}) },
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
  getBusRetained: (pattern: string) =>
    v2Call<{
      messages: Array<{
        topic: string;
        payload: unknown;
        origin?: string;
        retain?: boolean;
        seq?: number;
      }>;
    }>("GET", `/v2/bus/retained?pattern=${encodeURIComponent(pattern)}`),
  importMediaFile: (local_path: string) =>
    v2Call<{ ok: boolean; name: string; path: string }>("POST", "/v2/media/import", { local_path }),
  simulateKey: (key_index: number, depth_um = 2400, velocity_um_s = 15000, release = false) =>
    v2Call("POST", "/v2/simulate/key", { key_index, depth_um, velocity_um_s, release }),
  simulateImpulse: (x: number, y: number, radius = 64, force = 1.15) =>
    v2Call("POST", "/v2/simulate/impulse", { x, y, radius, force, kind: "ripple", source: "studio" }),
  simulateBurst: () => v2Call("POST", "/v2/simulate/burst", {}),
  syncToKeyboard: () =>
    v2Call<{
      ok: boolean;
      uploaded_files: string[];
      uploaded_bytes: number;
      skipped_unchanged?: number;
      persisted_verified?: boolean;
      persisted_error?: string | null;
      runtime_applied?: boolean | null;
    }>("POST", "/v2/keyboard/sync", {}),
  renderPreviewFrame: (steps = 2, step_ms = 16.667, width = 960, height = 540, overlay_only = false) =>
    v2Call<PreviewFrameResponse>("POST", "/v2/preview/frame", { steps, step_ms, width, height, overlay_only }),
  getMediaUrl: (name: string) => invoke<string>("v2_media_url", { name }),
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
  getPerf: () => v2Call<Record<string, unknown>>("GET", "/v2/perf"),
  getDisplayPrefs: () =>
    v2Call<{
      parallax: {
        parallaxActive: boolean;
        eyeHeight: number;
        eyeDistanceFromFront: number;
        footStandMode: "none" | "low" | "high";
        displayOffsetX: number;
        displayOffsetY: number;
        showAlignmentGrid: boolean;
      };
      power: {
        brightness: number;
        timeoutSec: number;
        followHostDisplay?: boolean;
      };
    }>("GET", "/v2/display/prefs"),
  putDisplayParallax: (parallax: Record<string, unknown>) =>
    v2Call<{ ok: boolean }>("PUT", "/v2/display/parallax", parallax),
  putDisplayPower: (power: { brightness: number; timeoutSec: number; followHostDisplay?: boolean }) =>
    v2Call<{ ok: boolean }>("PUT", "/v2/display/power", power),
  getDisplayAttachments: () => v2Call<KeyboardAttachmentsState>("GET", "/v2/display/attachments"),
  putDisplayAttachments: (bays: HardwareAttachmentBay[]) =>
    v2Call<{ ok: boolean; attachments: KeyboardAttachmentsState }>("PUT", "/v2/display/attachments", { bays }),
  getContextRules: () =>
    v2Call<{
      rules: ContextRule[];
      active_context?: { active: boolean; rule_id?: string; rule_name?: string; app_name?: string; qmk_layer?: number | null };
      current_app?: { process_name?: string; app_id?: string; window_title?: string } | null;
    }>("GET", "/v2/context/rules"),
  putContextRules: (rules: ContextRule[]) =>
    v2Call<{ ok: boolean; rules: ContextRule[] }>("PUT", "/v2/context/rules", { rules }),
  getHostIdentity: () => v2Call<HostIdentity>("GET", "/v2/host/identity"),
  putHostIdentity: (host: HostIdentity) =>
    v2Call<{ ok: boolean; host: HostIdentity }>("PUT", "/v2/host/identity", host),
  getProfiles: () =>
    v2Call<{
      manual_profile_id?: string | null;
      profiles: ProfileEntry[];
      active: ActiveProfileState;
      host: HostIdentity;
      current_app?: { process_name?: string; app_id?: string; window_title?: string } | null;
    }>("GET", "/v2/profiles"),
  putProfiles: (manual_profile_id: string | null, profiles: ProfileEntry[]) =>
    v2Call<{ ok: boolean; profiles: ProfileEntry[] }>("PUT", "/v2/profiles", {
      manual_profile_id,
      profiles,
    }),
  selectProfile: (profile_id: string | null) =>
    v2Call<{ ok: boolean; active?: ActiveProfileState }>("POST", "/v2/profiles/select", {
      profile_id,
    }),
  getStreamDeckState: () =>
    v2Call<{
      plugins: StreamDeckPluginInfo[];
      elgato_detected: StreamDeckPluginInfo[];
      buttons: StreamDeckButton[];
      faces: Record<string, { title?: string; image?: string; color?: string; state?: number }>;
      ws_port?: number;
    }>("GET", "/v2/streamdeck/state"),
  getStreamDeckPiUrl: (
    plugin_id: string,
    rel_path: string,
    ws_port: number,
    btn_id: string,
    action_uuid: string,
  ) =>
    invoke<string>("v2_streamdeck_pi_url", {
      pluginId: plugin_id,
      relPath: rel_path,
      wsPort: ws_port,
      btnId: btn_id,
      actionUuid: action_uuid,
    }),
  getStreamDeckPluginUrl: (plugin_id: string, rel_path: string, ws_port: number) =>
    invoke<string>("v2_streamdeck_plugin_url", {
      pluginId: plugin_id,
      relPath: rel_path,
      wsPort: ws_port,
    }),
  importStreamDeckPlugin: (path: string) =>
    v2Call<{ ok: boolean; plugin?: StreamDeckPluginInfo }>("POST", "/v2/streamdeck/import", { path }),
  deleteStreamDeckPlugin: (id: string) =>
    v2Call<{ ok: boolean; deleted: string }>("DELETE", `/v2/streamdeck/plugins/${encodeURIComponent(id)}`),
  putStreamDeckButtons: (buttons: StreamDeckButton[]) =>
    v2Call<{ ok: boolean; buttons: StreamDeckButton[] }>("PUT", "/v2/streamdeck/buttons", { buttons }),
  getKeyboardSoftwareStatus: () =>
    v2Call<{
      mode: "offline" | "stock" | "keyglass";
      connected: boolean;
      ofv2: boolean;
      versions: Record<string, unknown>;
    }>("GET", "/v2/keyboard/software-status"),
  returnKeyboardToStock: () =>
    v2Call<{ ok: boolean; message: string }>("POST", "/v2/keyboard/return-to-stock", {}),
  getLayerNames: () =>
    v2Call<{ layer_names: Record<string, string> }>("GET", "/v2/keymap/layer-names"),
  putLayerNames: (layer_names: Record<string, string>) =>
    v2Call<{ ok: boolean; layer_names: Record<string, string> }>("PUT", "/v2/keymap/layer-names", {
      layer_names,
    }),
};
