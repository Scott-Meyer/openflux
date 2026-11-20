export type KeyEffect = "none" | "glowing_rings" | "solid_highlight" | "ripple" | "autofocus_lock";

export interface KeyStyleOverride {
  style?: number;
  label?: string;
  iconAssetPath?: string;
  legendColor?: number;
  ringGlowColor?: number;
}

export interface StylemapLayer {
  defaultStyle?: KeyStyleOverride;
  styles: Record<string, KeyStyleOverride>;
}

export type LockIndicatorStyle = "off" | "dot" | "highlight";

export interface AppearanceTheme {
  id: string;
  name: string;
  fontFamily: string;
  fontWeight: number;
  fontItalic: boolean;
  backgroundImagePath: string;
  backgroundShaderParams?: { hue?: number; speed?: number };
  backgroundImageBlurAmount: number;
  backgroundImageDimming: number;
  moduleBackgroundDimming: number;
  selectedKeyEffect: KeyEffect;
  ringGlowColor: number;
  legendColor: number;
  lockIndicatorStyle?: LockIndicatorStyle;
  lockIndicatorColor?: number | null;
  retintIcons: boolean;
  retintModules: boolean;
  blendingModePreset: "off" | "dropShadow" | "lightGlow";
  stylemaps: Record<string, StylemapLayer>;
}

export interface AppearanceConfig {
  selectedId: string;
  defaultTheme: AppearanceTheme;
  customThemes: AppearanceTheme[];
}

export interface SettingsConfig {
  timeout: number;
  parallaxActive: boolean;
  eyeHeight: number;
  eyeDistanceFromFront: number;
  eyeXOffset: number;
  footStandMode: "none" | "low" | "high";
  displayOffsetX: number;
  displayOffsetY: number;
  showAlignmentGrid: boolean;
  locale: string;
}

export interface MosaicTileMeta {
  typeId: number;
  name: string;
  category: "Clock" | "System" | "Media" | "Lifestyle" | "Diagnostic" | "Custom";
  description: string;
}

export const ALL_MOSAIC_MODULES: MosaicTileMeta[] = [
  { typeId: 0, name: "Empty Slot", category: "Custom", description: "Blank Mosaic slot" },
  { typeId: 2, name: "Digital Clock", category: "Clock", description: "24h/12h clock with date" },
  { typeId: 3, name: "Debug Telemetry", category: "Diagnostic", description: "Scanner seq, FPS, video status" },
  { typeId: 4, name: "Device Info", category: "Diagnostic", description: "Active profile, layer, layout" },
  { typeId: 5, name: "QC Sensor Mode", category: "Diagnostic", description: "Hall sensor matrix diagnostics" },
  { typeId: 6, name: "Display Prefs", category: "Diagnostic", description: "Parallax, font, effect summary" },
  { typeId: 7, name: "Hardware Graph", category: "System", description: "Live CPU / GPU / RAM sparkline" },
  { typeId: 8, name: "Hardware Status", category: "System", description: "CPU, RAM, GPU & Net speeds" },
  { typeId: 9, name: "Keyboard State", category: "System", description: "Caps/Num/Scroll lock & active layer" },
  { typeId: 10, name: "Analog Clock", category: "Clock", description: "Sweep-hand analog watch face" },
  { typeId: 11, name: "Flip Clock", category: "Clock", description: "Split-flap retro clock" },
  { typeId: 12, name: "Minimal Clock", category: "Clock", description: "Clean typographic time + date" },
  { typeId: 13, name: "Pomodoro Timer", category: "Lifestyle", description: "Focus & break countdown timer" },
  { typeId: 14, name: "World Clock", category: "Clock", description: "Secondary timezone city clock" },
  { typeId: 15, name: "Weather", category: "Lifestyle", description: "Live forecast with a weather icon" },
  { typeId: 16, name: "Active Media", category: "Media", description: "Now playing title, artist & progress" },
  { typeId: 17, name: "Virtual Pet", category: "Lifestyle", description: "Animated pixel cat reacting to typing" },
  { typeId: 18, name: "Generic Module", category: "Custom", description: "API-driven graph or custom metric" },
];

export interface MosaicConfig {
  tileTypes: number[];
  tileConfigs: Record<string, unknown>[];
}

export interface KeyAction {
  keyCode?: string;
  type: number;
  layerControlType?: string;
  targetLayer?: number;
  macroId?: number;
  modifiers?: number | null;
}

export interface KeymapLayer {
  name: string;
  keyActions: KeyAction[];
}

export interface MacroStep {
  keyCode: string;
  delayMs: number;
  isDown: boolean;
}

export interface MacroDefinition {
  name: string;
  steps?: MacroStep[];
  words?: number[];
}

export interface KeymapConfig {
  selectedFrameLayoutType: "ansi" | "iso";
  keymaps: Record<string, { defaultLayer: number; layers: KeymapLayer[] }>;
  macros?: MacroDefinition[] | Record<string, unknown>;
}

export interface AppProfileRule {
  matcher: string;
  profile_index: number;
  language_code?: number | null;
}

/** Travel settings shared by an actuation mode and a per-key override, in mm from the top of the key. */
export interface ActuationTravel {
  actuationPointMm: number;
  rapidTrigger: boolean;
  rapidTriggerSensitivityMm?: number;
  /** Fixed actuation: where a held key releases. It is above the actuation point (less travel). */
  releasePointMm?: number;
  /** The keyboard's word for these settings when 0.1 mm steps can't express it; kept while they stay unchanged. */
  firmwareWord?: number;
  /** The keyboard has no setting here and uses its default fixed actuation. */
  firmwareDefault?: boolean;
}

export interface PerKeyActuationOverride extends ActuationTravel {
  resetToDefault?: boolean;
  touched?: boolean;
}

export interface ActuationMode extends ActuationTravel {
  id: string;
  name: string;
  /** 1: fixed actuation point, 2: rapid trigger. */
  type: 1 | 2;
  mixedPerKey?: boolean;
  globalTouched?: boolean;
  perKeyOverrides?: Record<string, Record<string, PerKeyActuationOverride>>;
  touched?: boolean;
}

export interface AssetCatalog {
  fonts: { family: string; files: string[]; hasItalic: boolean; isVariable: boolean }[];
  keyIcons: { id: string; category: string; name: string }[];
  media: { name: string; path: string; fullPath: string; kind: "video" | "image"; thumbnail?: string | null }[];
  presets: {
    id: string;
    name: string;
    fontFamily: string;
    fontWeight: number;
    fontItalic: boolean;
    backgroundImagePath: string;
    backgroundImageBlurAmount: number;
    backgroundImageDimming: number;
    moduleBackgroundDimming: number;
    selectedKeyEffect: KeyEffect;
    ringGlowColor: number;
    legendColor: number;
    blendingModePreset: "off" | "dropShadow" | "lightGlow";
    thumbnail: string;
  }[];
  shortcutPacks: { id: string; name: string; rowCount: number }[];
}

export interface ShortcutPackRow {
  context: string;
  shortcut: string;
  label: string;
  icon: string;
  macString: string;
  winString: string;
  macKeyCode?: string | null;
  macKeyCode2?: string | null;
  macHasSecondKey: boolean;
  macModifiers: number;
  winKeyCode?: string | null;
  winKeyCode2?: string | null;
  winHasSecondKey: boolean;
  winModifiers: number;
}

export interface InstalledAppInfo {
  bundleId: string;
  name: string;
  path: string;
}

/** Field names follow the profile file format (`profiles.json`), so they are snake_case. */
export interface ShortcutProfile {
  id: string;
  name: string;
  theme_id: string;
  contexts: string[];
  active_context: string;
  actuation_mode_index?: number;
  default_layer?: number;
  shortcut_pack?: string | null;
  app_matcher?: string | null;
}

export interface OpenFluxConfig {
  appearance: AppearanceConfig;
  settings: SettingsConfig;
  mosaic: MosaicConfig;
  keymap: KeymapConfig;
  appRules: AppProfileRule[];
  actuationModes?: ActuationMode[];
  profiles?: ShortcutProfile[];
  desiredProfileIndex?: number;
  explicitReset?: boolean;
  /** Patch flag: `appearance.selectedId` is a theme the user picked, which a save writes over one picked on the keyboard. */
  themeSelection?: boolean;
  hydrated?: {
    appearance: boolean;
    settings: boolean;
    mosaic: boolean;
    keymap: boolean;
  };
  /** The most layers per layout and actuation modes the keyboard holds; read-only. */
  keymapLimits?: { maxLayers: number; maxActuationModes: number };
}

/**
 * How the daemon's keymap file relates to the keyboard's firmware keymap:
 * - `ok`: they agree and the keymap can be edited;
 * - `pulling`: the daemon is reading the keymap from the keyboard;
 * - `mismatch`: the keymap file and the firmware keymap disagree;
 * - `error`: reading the keymap from the keyboard failed;
 * - `disconnected`: no keyboard is attached.
 */
export type KeymapSyncState = "ok" | "pulling" | "mismatch" | "error" | "disconnected";

export interface KeymapSync {
  state: KeymapSyncState;
  /** For `mismatch` and `error`, what went wrong in plain words. */
  reason: string | null;
}

/** Whether app rules may switch the profile: they do only while both are false. */
export interface AutoProfileSwitch {
  /** Automatic switching is locked, from Studio or the keyboard's lock key. */
  locked: boolean;
  /** A profile was picked on the keyboard: app rules wait until the foreground app changes. */
  heldUntilAppChange: boolean;
}

export interface DeviceStatus {
  connected: { bridge: boolean; firmware: boolean };
  /** Optional; omitted when profile-lock status is unavailable. */
  autoProfileSwitch?: AutoProfileSwitch;
  /** Optional; omitted when keymap sync status is unavailable. */
  keymapSync?: KeymapSync;
  versions: {
    firmware?: string | null;
    bridge?: string | null;
    display?: string | null;
    os?: string | null;
  };
  desiredProfileIndex: number;
  appliedProfileIndex?: number | null;
  desiredLanguageCode?: number | null;
  appliedLanguageCode?: number | null;
  hardware?: {
    cpu_pct: number;
    gpu_pct: number;
    mem_used_gb: number;
    mem_total_gb: number;
    net_down: string;
    net_up: string;
  };
  media?: {
    title: string;
    artists: string;
    album: string;
    playing: boolean;
  };
}

export interface PreviewKeyRect {
  keyIndex: number;
  x: number;
  y: number;
  w: number;
  h: number;
}

export const ANSI_ROW_COUNTS = [16, 15, 15, 14, 14, 11] as const;
export const ISO_ROW_COUNTS = [16, 15, 15, 15, 15, 11] as const;

/**
 * Shown until the first snapshot arrives from the daemon. Edits are blocked until
 * then, so these values are never written; the daemon owns the real defaults
 * (see `DaemonClient.getDefaultConfig`).
 */
export const PLACEHOLDER_CONFIG: OpenFluxConfig = {
  appearance: {
    selectedId: "default",
    defaultTheme: {
      id: "default",
      name: "Default",
      fontFamily: "Inter",
      fontWeight: 400,
      fontItalic: false,
      backgroundImagePath: "",
      backgroundImageBlurAmount: 0,
      backgroundImageDimming: 0,
      moduleBackgroundDimming: 0,
      selectedKeyEffect: "none",
      ringGlowColor: 0xffffffff,
      legendColor: 0xffffffff,
      retintIcons: false,
      retintModules: false,
      blendingModePreset: "off",
      stylemaps: {},
    },
    customThemes: [],
  },
  settings: {
    timeout: 0,
    parallaxActive: false,
    eyeHeight: 0,
    eyeDistanceFromFront: 0,
    eyeXOffset: 0,
    footStandMode: "none",
    displayOffsetX: 0,
    displayOffsetY: 0,
    showAlignmentGrid: false,
    locale: "en",
  },
  mosaic: { tileTypes: [0, 0, 0, 0], tileConfigs: [{}, {}, {}, {}] },
  keymap: { selectedFrameLayoutType: "ansi", keymaps: {} },
  appRules: [],
  profiles: [],
  actuationModes: [],
  desiredProfileIndex: 0,
};

export function argbToHexColor(argb: number): string {
  const rgb = (argb >>> 0) & 0xffffff;
  return `#${rgb.toString(16).padStart(6, "0")}`;
}

export function hexColorToArgb(hex: string): number {
  const clean = hex.replace("#", "").trim();
  const rgb = parseInt(clean, 16);
  if (Number.isNaN(rgb)) return 0xffffffff;
  return (0xff000000 | (rgb & 0xffffff)) >>> 0;
}

export function formatKeyActionLabel(
  ka?: { type?: number; targetLayer?: number; macroId?: number; keyCode?: string },
  fallbackIndex?: number,
): string {
  if (!ka) return fallbackIndex !== undefined ? `Key ${fallbackIndex + 1}` : "";
  if (ka.type === 2) return `FN${ka.targetLayer ?? 1}`;
  if (ka.type === 1) return `M${ka.macroId ?? 0}`;
  if (ka.type === 3) return "Reset";
  if (ka.type === 4) return "Pomo";
  const s = (ka.keyCode ?? "").replace(/^KC_/, "");
  const map: Record<string, string> = {
    ESCAPE: "Esc",
    BACKSPACE: "Bksp",
    CAPS_LOCK: "Caps",
    CAPS: "Caps",
    ENTER: "Enter",
    PAGE_UP: "PgUp",
    PAGE_DOWN: "PgDn",
    DELETE: "Del",
    LEFT_SHIFT: "LShift",
    LSHIFT: "LShift",
    RIGHT_SHIFT: "RShift",
    RSHIFT: "RShift",
    LEFT_CTRL: "LCtrl",
    LCTL: "LCtrl",
    RIGHT_CTRL: "RCtrl",
    RCTL: "RCtrl",
    LEFT_ALT: "Opt",
    LALT: "Opt",
    RIGHT_ALT: "ROpt",
    RALT: "ROpt",
    LEFT_GUI: "Cmd",
    LGUI: "Cmd",
    RIGHT_GUI: "RCmd",
    RGUI: "RCmd",
    LEFT_BRACKET: "[",
    RIGHT_BRACKET: "]",
    BACKSLASH: "\\",
    SEMICOLON: ";",
    QUOTE: "'",
    GRAVE: "`",
    COMMA: ",",
    DOT: ".",
    SLASH: "/",
    MINUS: "-",
    EQUAL: "=",
    SPACE: "Space",
    TAB: "Tab",
    LEFT: "←",
    DOWN: "↓",
    UP: "↑",
    RIGHT: "→",
    TRANSPARENT: "▽",
    // The same legends the keyboard draws.
    OF_RESET_POMODORO: "Rst",
    OF_START_PAUSE_POMODORO: "Pomo",
  };
  return map[s] ?? (s || (fallbackIndex !== undefined ? `Key ${fallbackIndex + 1}` : ""));
}

export function humanKeyName(config: OpenFluxConfig, keyIndex: number): string {
  const layout = config.keymap.selectedFrameLayoutType;
  const layer0 = config.keymap.keymaps[layout]?.layers?.[0];
  return formatKeyActionLabel(layer0?.keyActions?.[keyIndex], keyIndex);
}

/** True when the daemon reaches the keyboard through either the bridge or the firmware. */
export function isKeyboardConnected(status?: DeviceStatus | null): boolean {
  return Boolean(status?.connected.bridge || status?.connected.firmware);
}

/** What the keymap tabs say about the keyboard's keymap; the state is never `ok`. */
export interface KeymapSyncNotice {
  state: Exclude<KeymapSyncState, "ok">;
  reason: string | null;
  /** True while keymap edits (keys, layers, macros, actuation, shortcut packs) wait for the keyboard. */
  locksEditing: boolean;
}

/**
 * What to tell the user about the keyboard's keymap, or `null` when it is loaded. Editing waits
 * while a keyboard is attached and the daemon hasn't loaded its keymap.
 */
export function keymapSyncNotice(
  status: DeviceStatus | null | undefined,
  hydrated: OpenFluxConfig["hydrated"],
): KeymapSyncNotice | null {
  if (!status || !hydrated || hydrated.keymap) return null;
  if (!isKeyboardConnected(status)) return { state: "disconnected", reason: null, locksEditing: false };
  const sync = status.keymapSync;
  switch (sync?.state) {
    case "mismatch":
    case "error":
    case "disconnected":
      return { state: sync.state, reason: sync.reason ?? null, locksEditing: true };
    default:
      // `pulling`, a daemon that doesn't report sync, or an `ok` the config snapshot hasn't caught up with.
      return { state: "pulling", reason: null, locksEditing: true };
  }
}
