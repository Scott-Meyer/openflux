import type { KeyAction, KeymapLayer, KeyglassConfig } from "../types/keyglass";

// A complete configuration for UI tests, shaped like the daemon's GET /v1/config response.

const FN_KEY: KeyAction = { type: 2, layerControlType: "momentaryLayer", targetLayer: 1 };

const ANSI_BASE = [
  "ESCAPE F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 F13 F14 DELETE",
  "GRAVE 1 2 3 4 5 6 7 8 9 0 MINUS EQUAL BACKSPACE HOME",
  "TAB Q W E R T Y U I O P LEFT_BRACKET RIGHT_BRACKET BACKSLASH END",
  "CAPS_LOCK A S D F G H J K L SEMICOLON QUOTE ENTER PAGE_UP",
  "LEFT_SHIFT Z X C V B N M COMMA DOT SLASH RIGHT_SHIFT UP PAGE_DOWN",
  "LEFT_CTRL LEFT_ALT LEFT_GUI SPACE RIGHT_GUI FN RIGHT_ALT RIGHT_CTRL LEFT DOWN RIGHT",
];

const ISO_BASE = [
  "ESCAPE F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 F13 F14 DELETE",
  "GRAVE 1 2 3 4 5 6 7 8 9 0 MINUS EQUAL BACKSPACE HOME",
  "TAB Q W E R T Y U I O P LEFT_BRACKET RIGHT_BRACKET ENTER END",
  "CAPS_LOCK A S D F G H J K L SEMICOLON QUOTE NONUS_HASH ENTER PAGE_UP",
  "LEFT_SHIFT NONUS_BACKSLASH Z X C V B N M COMMA DOT SLASH RIGHT_SHIFT UP PAGE_DOWN",
  "LEFT_CTRL LEFT_ALT LEFT_GUI SPACE RIGHT_GUI FN RIGHT_ALT RIGHT_CTRL LEFT DOWN RIGHT",
];

const FN_OVERRIDES: Record<number, string> = {
  1: "BRIGHTNESS_DOWN",
  2: "BRIGHTNESS_UP",
  7: "MEDIA_PREV_TRACK",
  8: "MEDIA_PLAY_PAUSE",
  9: "MEDIA_NEXT_TRACK",
  10: "AUDIO_MUTE",
  11: "AUDIO_VOL_DOWN",
  12: "AUDIO_VOL_UP",
  20: "OF_PROFILE_CYCLE_NEXT",
  23: "OF_ACTUATION_MODE_NEXT",
  25: "OF_THEME_CYCLE_NEXT",
};

function key(code: string): KeyAction {
  return code === "FN" ? { ...FN_KEY } : { keyCode: `KC_${code}`, type: 0, modifiers: null };
}

function layers(rows: string[]): KeymapLayer[] {
  const codes = rows.join(" ").split(" ");
  return [
    { name: "Base", keyActions: codes.map(key) },
    {
      name: "FN",
      keyActions: codes.map((_, i) => key(FN_OVERRIDES[i] ?? "TRANSPARENT")),
    },
  ];
}

export const TEST_CONFIG: KeyglassConfig = {
  appearance: {
    selectedId: "default",
    defaultTheme: {
      id: "default",
      name: "Test Theme",
      fontFamily: "Inter",
      fontWeight: 300,
      fontItalic: false,
      backgroundImagePath: "aurora_drift.frag",
      backgroundImageBlurAmount: 0,
      backgroundImageDimming: 20,
      moduleBackgroundDimming: 0,
      selectedKeyEffect: "glowing_rings",
      ringGlowColor: 0xff66ccff,
      legendColor: 0xffffffff,
      retintIcons: true,
      retintModules: true,
      blendingModePreset: "dropShadow",
      stylemaps: {},
    },
    customThemes: [],
  },
  settings: {
    timeout: 900,
    parallaxActive: true,
    eyeHeight: 425,
    eyeDistanceFromFront: 225,
    eyeXOffset: 0,
    footStandMode: "none",
    displayOffsetX: 0,
    displayOffsetY: 0,
    showAlignmentGrid: false,
    locale: "en",
  },
  mosaic: {
    tileTypes: [12, 8, 15, 13],
    tileConfigs: [
      {},
      {},
      { identifier: 15, city: "Lisbon", region: "", country: "Portugal", celcius: true },
      { identifier: 13, main: "25", shortBreak: "5", longBreak: "15", breakCount: "4", color: "Legend" },
    ],
  },
  keymap: {
    selectedFrameLayoutType: "ansi",
    keymaps: {
      ansi: { defaultLayer: 0, layers: layers(ANSI_BASE) },
      iso: { defaultLayer: 0, layers: layers(ISO_BASE) },
    },
    macros: {},
  },
  appRules: [],
  profiles: [
    {
      id: "default",
      name: "Default",
      theme_id: "default",
      contexts: ["Main"],
      active_context: "Main",
      actuation_mode_index: 0,
      default_layer: 0,
      shortcut_pack: null,
      app_matcher: null,
    },
    {
      id: "coding",
      name: "VS Code",
      theme_id: "default",
      contexts: ["Editor", "Debug", "Terminal"],
      active_context: "Editor",
      actuation_mode_index: 0,
      default_layer: 0,
      shortcut_pack: "Visual%20Studio%20Code.csv",
      app_matcher: "com.microsoft.VSCode",
    },
  ],
  desiredProfileIndex: 0,
  actuationModes: [
    { id: "typing", name: "Typing/General", type: 1, actuationPointMm: 2.0, rapidTrigger: false },
    {
      id: "gaming",
      name: "FPS Gaming",
      type: 2,
      actuationPointMm: 1.2,
      rapidTrigger: true,
      rapidTriggerSensitivityMm: 0.3,
    },
  ],
};
