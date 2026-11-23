import { invoke } from "@tauri-apps/api/core";
import type {
  AssetCatalog,
  AutoProfileSwitch,
  DeviceStatus,
  InstalledAppInfo,
  OpenFluxConfig,
  PreviewKeyRect,
  ShortcutPackRow,
} from "../types/openflux";
import { toDaemonError } from "./daemonError";

/** A preview frame request. Studio's preview keeps its own clock. */
export interface RenderPreviewOptions {
  suspended?: boolean;
  halfRes?: boolean;
  liveJson?: Record<string, unknown>;
  config?: OpenFluxConfig;
}

export interface AerialVideoItem {
  id: string;
  title: string;
  quality: string;
  thumbnailUrl: string;
  videoUrl: string;
}

export interface SocStatsSample {
  cpuPct?: number;
  gpuPct?: number | null;
  memUsedMb?: number;
  memTotalMb?: number;
  renderMs?: number;
  rkmpp?: boolean;
  tsSec?: number;
  sampleMs?: number;
  offline?: boolean;
}

export interface ImportedMedia {
  assetPath: string;
  thumbnail?: string | null;
  isVideo: boolean;
  transcoded: boolean;
  /** Length of the imported file in seconds. */
  durationS?: number | null;
  /** False when the length couldn't be read. */
  durationKnown?: boolean;
  /** Length of the original file in seconds, when known. */
  sourceDurationS?: number | null;
  /** Resolution of the imported video, or `original` when it was kept as it was. */
  resolution?: string;
  /** Video bitrate chosen to fit the keyboard's media limit, when the daemon reports it. */
  bitrateKbps?: number | null;
}

/** Largest media file the keyboard accepts. */
export const KEYBOARD_MEDIA_LIMIT_MIB = 96;

export interface PomodoroResult {
  message: string;
  pomodoro?: { running: boolean; remainingS: number; defaultS?: number; rev?: number };
}

/** What the keyboard firmware has stored for press refinement. */
export type PressRefinementState = "absent" | "installed" | "invalid" | "storageUnavailable" | "unknown";

/** `GET /v1/keyboard/press-refinement`. */
export interface PressRefinementStatus {
  state: PressRefinementState;
  /** CRC-32 of the installed data; `null` unless installed. */
  crc32: number | null;
  /** For installed data: whether the firmware applies it, or only stores it. */
  active?: boolean;
  /** Why the state is `unknown`. */
  code?: string;
  message?: string;
}

/** `POST /v1/keyboard/press-refinement/install`. */
export interface PressRefinementInstall extends PressRefinementStatus {
  /** The keyboard already had exactly this data. */
  alreadyInstalled: boolean;
  /** The data came from the copy Keyglass keeps rather than a download. */
  fromCache: boolean;
}

/** The keyboard's press-refinement data. Both reject with a `DaemonError`. */
export interface PressRefinementClient {
  getPressRefinement(): Promise<PressRefinementStatus>;
  /**
   * Installs and confirms the data. Without `path` it is the kept copy or a download from the
   * maker's published software; with `path` it comes from that file (the maker's `.msix` package
   * or the bare `.uf2`). It is verified either way.
   */
  installPressRefinement(path?: string): Promise<PressRefinementInstall>;
}

/**
 * Everything Studio asks of the daemon and the desktop shell. The app runs inside
 * Tauri, so every call goes through the Rust side (`src-tauri`), which owns the
 * daemon connection, enrollment and the local display preview.
 */
export interface DaemonClient extends PressRefinementClient {
  getStatus(): Promise<DeviceStatus>;
  /** Locks or unlocks automatic profile switching; answers with the new state. */
  setAutoProfileLock(locked: boolean): Promise<AutoProfileSwitch>;
  getConfig(): Promise<OpenFluxConfig>;
  /** The daemon's default configuration, in the same shape as `getConfig`. */
  getDefaultConfig(): Promise<OpenFluxConfig>;
  updateConfig(patch: Partial<OpenFluxConfig>): Promise<void>;
  saveToKeyboard(): Promise<{ message: string }>;
  /**
   * Rebuilds the keymap file from the keyboard's firmware keymap, writes it to the keyboard and
   * answers with the new config. Rejects with a `DaemonError`: a 409 when there is no readable
   * firmware keymap, other keyboard work is running, or the keyboard needs conversion.
   */
  adoptFirmwareKeymap(): Promise<OpenFluxConfig>;
  importBundle(filePath: string): Promise<{ message: string; profiles: string[] }>;
  exportBundle(outPath: string): Promise<{ message: string; outPath: string }>;
  addIcon(filePath: string): Promise<{ message: string; iconPaths: string[] }>;
  importMedia(filePath: string, opts?: { loopReverse?: boolean }): Promise<ImportedMedia>;
  getAerialVideos(): Promise<AerialVideoItem[]>;
  importAerialVideo(url: string, title?: string, opts?: { loopReverse?: boolean }): Promise<ImportedMedia>;
  getAssetsCatalog(): Promise<AssetCatalog>;
  getShortcutPack(pack: string): Promise<{ pack: string; application: string; rows: ShortcutPackRow[] }>;
  getAssetDataUrl(cat: string, name: string): Promise<string>;
  getInstalledApps(): Promise<InstalledAppInfo[]>;
  getPreviewRects(layout?: string): Promise<PreviewKeyRect[]>;
  renderPreviewRgba(opts?: RenderPreviewOptions): Promise<Uint8Array>;
  closePreview(): Promise<void>;
  backupDevice(outDir: string): Promise<{ message: string }>;
  flashFirmwareUf2(uf2Path: string): Promise<{ message: string }>;
  sideloadIpk(ipkPath: string): Promise<{ message: string }>;
  pomodoroAction(action: "toggle" | "reset"): Promise<PomodoroResult>;
  getSocStats(): Promise<SocStatsSample>;
  pickFile(title?: string, extensions?: string[]): Promise<string | null>;
  pickSaveFile(title?: string, defaultName?: string, extensions?: string[]): Promise<string | null>;
  pickFolder(title?: string): Promise<string | null>;
}

export function createDaemonClient(): DaemonClient {
  return {
    getStatus: () => invoke<DeviceStatus>("get_status"),
    setAutoProfileLock: (locked) => invoke<AutoProfileSwitch>("set_auto_profile_lock", { locked }),
    getConfig: () => invoke<OpenFluxConfig>("get_config"),
    getDefaultConfig: () => invoke<OpenFluxConfig>("get_default_config"),
    updateConfig: (patch) => invoke("update_config", { patch }),
    saveToKeyboard: () => invoke("save_to_keyboard"),
    adoptFirmwareKeymap: () =>
      invoke<OpenFluxConfig>("adopt_firmware_keymap").catch((e) => {
        throw toDaemonError(e);
      }),
    importBundle: (filePath) => invoke("import_bundle", { filePath }),
    exportBundle: (outPath) => invoke("export_bundle", { outPath }),
    addIcon: (filePath) => invoke("add_icon", { filePath }),
    importMedia: (filePath, opts) =>
      invoke<ImportedMedia>("import_media", { filePath, loopReverse: opts?.loopReverse ?? false }).catch((e) => {
        throw toDaemonError(e);
      }),
    getAerialVideos: async () => {
      const res = await invoke<{ videos?: AerialVideoItem[] }>("get_aerial_videos");
      return res.videos ?? [];
    },
    importAerialVideo: (url, title, opts) =>
      invoke<ImportedMedia>("import_aerial_video", {
        url,
        title,
        loopReverse: opts?.loopReverse ?? false,
      }).catch((e) => {
        throw toDaemonError(e);
      }),
    getAssetsCatalog: () => invoke<AssetCatalog>("get_assets_catalog"),
    getShortcutPack: (pack) => invoke("get_shortcut_pack", { pack }),
    getAssetDataUrl: (cat, name) => invoke<string>("read_asset_data_url", { cat, name }),
    getInstalledApps: async () => {
      const res = await invoke<{ apps: InstalledAppInfo[] }>("get_installed_apps");
      return res.apps;
    },
    getPreviewRects: (layout) => invoke<PreviewKeyRect[]>("preview_rects", { layout }),
    renderPreviewRgba: async (opts) => {
      const res = await invoke<ArrayBuffer | Uint8Array | number[]>("preview_render_rgba", { opts: opts ?? {} });
      return res instanceof Uint8Array ? res : new Uint8Array(res);
    },
    closePreview: () => invoke("preview_close"),
    backupDevice: (outDir) => invoke("backup_device", { outDir }),
    flashFirmwareUf2: (uf2Path) => invoke("flash_uf2", { uf2Path }),
    sideloadIpk: (ipkPath) => invoke("sideload_ipk", { ipkPath }),
    pomodoroAction: (action) => invoke("pomodoro_action", { action }),
    getSocStats: () => invoke<SocStatsSample>("get_soc_stats"),
    pickFile: (title, extensions) => invoke<string | null>("pick_file", { title, extensions }),
    pickSaveFile: (title, defaultName, extensions) =>
      invoke<string | null>("pick_save_file", { title, defaultName, extensions }),
    pickFolder: (title) => invoke<string | null>("pick_folder", { title }),
    getPressRefinement: () =>
      invoke<PressRefinementStatus>("get_press_refinement").catch((e) => {
        throw toDaemonError(e);
      }),
    installPressRefinement: (path) =>
      invoke<PressRefinementInstall>("install_press_refinement", { path: path ?? null }).catch((e) => {
        throw toDaemonError(e);
      }),
  };
}
