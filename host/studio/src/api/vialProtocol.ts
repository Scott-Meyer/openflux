/**
 * Native Vial / VIA Raw HID (`0xFF60:0x61`) protocol client & QMK keycode codec.
 *
 * Talks directly through `openfluxd`'s authenticated `/v2/vial/xfer` endpoint — no Vialite iframe,
 * no unrelated RGB/Backlight menus, 100% cohesive native OpenFlux Studio experience.
 */
import { v2Client } from "./v2Client";

export const MATRIX_ROWS = 6;
export const MATRIX_COLS = 16;

// VIA / Vial Raw HID command opcodes (32-byte packets)
const ID_GET_KEYCODE = 0x04;
const ID_SET_KEYCODE = 0x05;
const ID_DYNAMIC_KEYMAP_GET_LAYER_COUNT = 0x11;
const ID_DYNAMIC_KEYMAP_GET_BUFFER = 0x12;

export interface ActionSlotInfo {
  slot: number;
  keycode: number;
  action_id: string;
  module_id?: string;
  label?: string;
  short_label?: string;
  icon?: string;
  accent?: string;
  mode?: string;
}

export interface DecodedKeycode {
  label: string;
  sub?: string;
  kind: "basic" | "mod" | "layer" | "hold_tap" | "action" | "gamepad" | "trns" | "none";
  accent?: string;
}

export interface KeycodeOption {
  code: number;
  label: string;
  sub?: string;
  group: string;
}

export const ANALOG_GAMEPAD_KEYS: Array<{ code: number; label: string; name: string; group: string }> = [
  { code: 0x7e27, label: "LS↑", name: "Left Stick Up", group: "Left Stick" },
  { code: 0x7e28, label: "LS↓", name: "Left Stick Down", group: "Left Stick" },
  { code: 0x7e29, label: "LS←", name: "Left Stick Left", group: "Left Stick" },
  { code: 0x7e2a, label: "LS→", name: "Left Stick Right", group: "Left Stick" },
  { code: 0x7e2b, label: "RS↑", name: "Right Stick Up", group: "Right Stick" },
  { code: 0x7e2c, label: "RS↓", name: "Right Stick Down", group: "Right Stick" },
  { code: 0x7e2d, label: "RS←", name: "Right Stick Left", group: "Right Stick" },
  { code: 0x7e2e, label: "RS→", name: "Right Stick Right", group: "Right Stick" },
  { code: 0x7e2f, label: "LT", name: "Left Trigger (Analog)", group: "Triggers & Bumpers" },
  { code: 0x7e30, label: "RT", name: "Right Trigger (Analog)", group: "Triggers & Bumpers" },
  { code: 0x7e35, label: "LB", name: "Left Bumper", group: "Triggers & Bumpers" },
  { code: 0x7e36, label: "RB", name: "Right Bumper", group: "Triggers & Bumpers" },
  { code: 0x7e31, label: "A", name: "Button A (South)", group: "Face Buttons" },
  { code: 0x7e32, label: "B", name: "Button B (East)", group: "Face Buttons" },
  { code: 0x7e33, label: "X", name: "Button X (West)", group: "Face Buttons" },
  { code: 0x7e34, label: "Y", name: "Button Y (North)", group: "Face Buttons" },
  { code: 0x7e37, label: "L3", name: "Left Stick Click", group: "System & Sticks" },
  { code: 0x7e38, label: "R3", name: "Right Stick Click", group: "System & Sticks" },
  { code: 0x7e3d, label: "Start", name: "Start / Menu", group: "System & Sticks" },
  { code: 0x7e3e, label: "Back", name: "Back / View", group: "System & Sticks" },
  { code: 0x7e3f, label: "Guide", name: "Guide / Home", group: "System & Sticks" },
  { code: 0x7e39, label: "D↑", name: "D-Pad Up", group: "D-Pad" },
  { code: 0x7e3a, label: "D↓", name: "D-Pad Down", group: "D-Pad" },
  { code: 0x7e3b, label: "D←", name: "D-Pad Left", group: "D-Pad" },
  { code: 0x7e3c, label: "D→", name: "D-Pad Right", group: "D-Pad" },
];

const GP_BY_CODE = new Map(ANALOG_GAMEPAD_KEYS.map((g) => [g.code, g]));

const BUILTIN_FLUX_ACTIONS: Record<number, { label: string; name: string }> = {
  0: { label: "Bright+", name: "Display Brightness Up" },
  1: { label: "Bright-", name: "Display Brightness Down" },
  2: { label: "Prof+", name: "Next Profile" },
  3: { label: "Prof-", name: "Previous Profile" },
  4: { label: "Act+", name: "Next Actuation Mode" },
  5: { label: "Act-", name: "Previous Actuation Mode" },
  6: { label: "Linux", name: "OS Mode: Linux" },
  7: { label: "macOS", name: "OS Mode: macOS" },
  8: { label: "Win", name: "OS Mode: Windows" },
  9: { label: "Lock", name: "Auto-Profile Lock" },
  10: { label: "Pomo", name: "Pomodoro Start / Pause" },
  11: { label: "Reset", name: "Pomodoro Reset" },
  12: { label: "Theme+", name: "Next Theme" },
  13: { label: "Theme-", name: "Previous Theme" },
};

const HID_LABELS: Record<number, string> = {
  0x1e: "1", 0x1f: "2", 0x20: "3", 0x21: "4", 0x22: "5",
  0x23: "6", 0x24: "7", 0x25: "8", 0x26: "9", 0x27: "0",
  0x28: "Enter", 0x29: "Esc", 0x2a: "Bksp", 0x2b: "Tab", 0x2c: "Space",
  0x2d: "-", 0x2e: "=", 0x2f: "[", 0x30: "]", 0x31: "\\",
  0x32: "#", 0x33: ";", 0x34: "'", 0x35: "`", 0x36: ",", 0x37: ".", 0x38: "/",
  0x39: "Caps",
  0x3a: "F1", 0x3b: "F2", 0x3c: "F3", 0x3d: "F4", 0x3e: "F5", 0x3f: "F6",
  0x40: "F7", 0x41: "F8", 0x42: "F9", 0x43: "F10", 0x44: "F11", 0x45: "F12",
  0x46: "PrtSc", 0x47: "ScrLk", 0x48: "Pause", 0x49: "Ins",
  0x4a: "Home", 0x4b: "PgUp", 0x4c: "Del", 0x4d: "End", 0x4e: "PgDn",
  0x4f: "→", 0x50: "←", 0x51: "↓", 0x52: "↑",
  0x68: "F13", 0x69: "F14", 0x6a: "F15", 0x6b: "F16", 0x6c: "F17", 0x6d: "F18", 0x6e: "F19", 0x6f: "F20",
  0xa8: "Mute", 0xa9: "Vol+", 0xaa: "Vol-", 0xab: "Next", 0xac: "Prev", 0xad: "Stop", 0xae: "Play",
  0xbd: "Bri+", 0xbe: "Bri-", 0xc1: "Mission", 0xc2: "Launch",
  0xe0: "LCtrl", 0xe1: "LShift", 0xe2: "LOpt", 0xe3: "LCmd",
  0xe4: "RCtrl", 0xe5: "RShift", 0xe6: "ROpt", 0xe7: "RCmd",
};

export const HOLD_MOD_OPTIONS: Array<{ bit: number; label: string; short: string }> = [
  { bit: 0x01, label: "Left Ctrl", short: "Ctrl" },
  { bit: 0x02, label: "Left Shift", short: "Shift" },
  { bit: 0x04, label: "Left Opt / Alt", short: "Opt" },
  { bit: 0x08, label: "Left Cmd / GUI", short: "Cmd" },
  { bit: 0x11, label: "Right Ctrl", short: "RCtl" },
  { bit: 0x12, label: "Right Shift", short: "RSft" },
  { bit: 0x14, label: "Right Opt / Alt", short: "ROpt" },
  { bit: 0x18, label: "Right Cmd / GUI", short: "RCmd" },
];

export const LAYER_OP_OPTIONS: Array<{ base: number; prefix: string; name: string; desc: string }> = [
  { base: 0x5220, prefix: "MO", name: "Hold (MO)", desc: "Active while key is held" },
  { base: 0x5260, prefix: "TG", name: "Toggle (TG)", desc: "Tap to turn layer on or off" },
  { base: 0x5240, prefix: "DF", name: "Default (DF)", desc: "Switch base default layer" },
  { base: 0x5280, prefix: "OSL", name: "One-Shot (OSL)", desc: "Active for next keypress" },
  { base: 0x52c0, prefix: "TT", name: "Tap-Toggle (TT)", desc: "Hold for momentary, tap to toggle" },
];

export const KEY_PICKER_GROUPS: Array<{ group: string; keys: Array<{ code: number; label: string }> }> = [
  {
    group: "Special",
    keys: [
      { code: 0x0001, label: "▽ Trans" },
      { code: 0x0000, label: "✕ None" },
      { code: 0x7c73, label: "CapsWord" },
    ],
  },
  {
    group: "Letters",
    keys: Array.from({ length: 26 }, (_, i) => ({
      code: 0x04 + i,
      label: String.fromCharCode(65 + i),
    })),
  },
  {
    group: "Numbers & Symbols",
    keys: [
      0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
      0x35, 0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x33, 0x34, 0x36, 0x37, 0x38,
    ].map((c) => ({ code: c, label: HID_LABELS[c] })),
  },
  {
    group: "Modifiers & Editing",
    keys: [
      0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,
      0x29, 0x2b, 0x39, 0x28, 0x2a, 0x2c, 0x4c, 0x49,
    ].map((c) => ({ code: c, label: HID_LABELS[c] })),
  },
  {
    group: "Navigation & Arrows",
    keys: [0x52, 0x51, 0x50, 0x4f, 0x4a, 0x4d, 0x4b, 0x4e, 0x46, 0x47, 0x48].map((c) => ({
      code: c,
      label: HID_LABELS[c],
    })),
  },
  {
    group: "Function Keys",
    keys: [
      0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45,
      0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f,
    ].map((c) => ({ code: c, label: HID_LABELS[c] })),
  },
  {
    group: "Media & System",
    keys: [0xa8, 0xaa, 0xa9, 0xac, 0xae, 0xab, 0xad, 0xbe, 0xbd, 0xc1, 0xc2].map((c) => ({
      code: c,
      label: HID_LABELS[c],
    })),
  },
];

/** Maps browser `KeyboardEvent.code` to standard USB HID Usage ID for instant "Press a key" capture! */
export function domCodeToHid(code: string): number | null {
  if (/^Key[A-Z]$/.test(code)) return 0x04 + (code.charCodeAt(3) - 65);
  if (/^Digit[1-9]$/.test(code)) return 0x1e + (code.charCodeAt(5) - 49);
  if (code === "Digit0") return 0x27;
  if (/^F([1-9]|1[0-2])$/.test(code)) return 0x3a + (Number(code.slice(1)) - 1);
  const map: Record<string, number> = {
    Enter: 0x28, Escape: 0x29, Backspace: 0x2a, Tab: 0x2b, Space: 0x2c,
    Minus: 0x2d, Equal: 0x2e, BracketLeft: 0x2f, BracketRight: 0x30, Backslash: 0x31,
    Semicolon: 0x33, Quote: 0x34, Backquote: 0x35, Comma: 0x36, Period: 0x37, Slash: 0x38,
    CapsLock: 0x39, PrintScreen: 0x46, ScrollLock: 0x47, Pause: 0x48, Insert: 0x49,
    Home: 0x4a, PageUp: 0x4b, Delete: 0x4c, End: 0x4d, PageDown: 0x4e,
    ArrowRight: 0x4f, ArrowLeft: 0x50, ArrowDown: 0x51, ArrowUp: 0x52,
    ControlLeft: 0xe0, ShiftLeft: 0xe1, AltLeft: 0xe2, MetaLeft: 0xe3,
    ControlRight: 0xe4, ShiftRight: 0xe5, AltRight: 0xe6, MetaRight: 0xe7,
  };
  return map[code] ?? null;
}

export function encodeModTap(modBit: number, tapHid: number): number {
  return 0x2000 | ((modBit & 0x1f) << 8) | (tapHid & 0xff);
}

export function encodeLayerTap(layer: number, tapHid: number): number {
  return 0x4000 | ((layer & 0x0f) << 8) | (tapHid & 0xff);
}

export function encodeLayerOp(base: number, layer: number): number {
  return base | (layer & 0x1f);
}

/** Encodes a modified QMK keycode (`0x0100..0x1FFF`: e.g. `S(KC_1)` = `!`, `C(S(G(KC_4)))` = screenshot). */
export function encodeModifiedKey(modMask: number, tapHid: number): number {
  return ((modMask & 0x1f) << 8) | (tapHid & 0xff);
}

export function encodeTapDance(index: number): number {
  return 0x5700 | (index & 0xff);
}

export function decodeQmkKeycode(
  kc: number,
  actionsBySlot: Record<number, ActionSlotInfo> = {},
): DecodedKeycode {
  if (kc === 0x0000) return { label: "✕", kind: "none" };
  if (kc === 0x0001) return { label: "▽", kind: "trns" };
  if (kc >= 0x04 && kc <= 0x1d) {
    return { label: String.fromCharCode(65 + (kc - 0x04)), kind: "basic" };
  }
  if (kc <= 0xff) {
    const lbl = HID_LABELS[kc] ?? `0x${kc.toString(16).toUpperCase()}`;
    const isMod = kc >= 0xe0 && kc <= 0xe7;
    return { label: lbl, kind: isMod ? "mod" : "basic" };
  }
  // Modified keycodes: 0x0100..0x1FFF (S(kc), C(kc), A(kc), G(kc), combos like Ctrl+Shift+Cmd+4!)
  if (kc >= 0x0100 && kc <= 0x1fff) {
    const mods = (kc >> 8) & 0x1f;
    const base = kc & 0xff;
    const baseDec = decodeQmkKeycode(base, actionsBySlot);
    const pfx: string[] = [];
    if (mods & 1) pfx.push("⌃");
    if (mods & 2) pfx.push("⇧");
    if (mods & 4) pfx.push("⌥");
    if (mods & 8) pfx.push("⌘");
    return { label: `${pfx.join("")}${baseDec.label}`, sub: "Mod", kind: "mod", accent: "#60a5fa" };
  }
  // Mod-Tap: MT(mod, kc)
  if (kc >= 0x2000 && kc <= 0x3fff) {
    const tap = decodeQmkKeycode(kc & 0xff, actionsBySlot);
    const modBits = (kc >> 8) & 0x1f;
    const m = HOLD_MOD_OPTIONS.find((x) => x.bit === modBits);
    return { label: tap.label, sub: m?.short ?? "Mod", kind: "hold_tap", accent: "#a78bfa" };
  }
  // Layer-Tap: LT(layer, kc)
  if (kc >= 0x4000 && kc <= 0x4fff) {
    const tap = decodeQmkKeycode(kc & 0xff, actionsBySlot);
    const lyr = (kc >> 8) & 0x0f;
    return { label: tap.label, sub: `L${lyr}`, kind: "hold_tap", accent: "#a78bfa" };
  }
  // Layer keys: MO / DF / TG / OSL / TT
  if (kc >= 0x5220 && kc <= 0x52ff) {
    const lyr = kc & 0x1f;
    if (kc < 0x5240) return { label: lyr === 1 ? "Fn" : `MO(${lyr})`, sub: `L${lyr}`, kind: "layer", accent: "#c084fc" };
    if (kc < 0x5260) return { label: `DF(${lyr})`, kind: "layer", accent: "#c084fc" };
    if (kc < 0x5280) return { label: `TG(${lyr})`, kind: "layer", accent: "#c084fc" };
    if (kc < 0x52a0) return { label: `OSL(${lyr})`, kind: "layer", accent: "#c084fc" };
    return { label: `TT(${lyr})`, kind: "layer", accent: "#c084fc" };
  }
  // Tap Dance: TD(n) (0x5700..0x57FF)
  if (kc >= 0x5700 && kc <= 0x57ff) {
    return { label: `TD(${kc & 0xff})`, sub: "Dance", kind: "hold_tap", accent: "#f472b6" };
  }
  // Fixed 25-slot Analog Gamepad block (0x7E27..0x7E3F)
  const gp = GP_BY_CODE.get(kc);
  if (gp) {
    return { label: gp.label, sub: "Pad", kind: "gamepad", accent: "#58f29c" };
  }
  // QK_USER_31 is the firmware's macOS Globe key, not an action.
  if (kc === 0x7e5f) return { label: "Globe", kind: "basic" };
  // Actions: QK_KB_0..63 (0x7E00..0x7E3F) + QK_USER (0x7E40..0x7FFF), slot = keycode - 0x7E00.
  // Firmware before 2.1.6 only forwards QK_USER up to 0x7E5E.
  if (kc >= 0x7e00 && kc <= 0x7fff) {
    const slot = kc - 0x7e00;
    const act = actionsBySlot[slot];
    if (act && slot >= 14) {
      const lbl = act.short_label || act.label || act.action_id.split(".").pop() || `Act${slot}`;
      return {
        label: lbl,
        sub: act.mode === "hold" ? "Hold" : undefined,
        kind: "action",
        accent: act.accent || "#fa6831",
      };
    }
    const bi = BUILTIN_FLUX_ACTIONS[slot];
    if (bi) return { label: bi.label, kind: "action", accent: "#7ae7ff" };
    return { label: `Act${slot}`, kind: "action", accent: "#fa6831" };
  }
  if (kc === 0x7c73) return { label: "CW", kind: "mod" };
  return { label: `0x${kc.toString(16).toUpperCase()}`, kind: "basic" };
}

async function xfer(packet: number[]): Promise<number[]> {
  const padded = new Array(32).fill(0);
  for (let i = 0; i < Math.min(32, packet.length); i++) padded[i] = packet[i] & 0xff;
  const res = await v2Client.vialXfer(padded);
  return res.data;
}

export async function vialGetLayerCount(): Promise<number> {
  const resp = await xfer([ID_DYNAMIC_KEYMAP_GET_LAYER_COUNT]);
  return Math.max(1, Math.min(8, resp[1] || 8));
}

/** Reads the entire live `[layer][row][col]` keymap matrix from RP2040 Vial EEPROM. */
export async function vialReadAllLayers(
  rows = MATRIX_ROWS,
  cols = MATRIX_COLS,
): Promise<{ layerCount: number; keymaps: number[][][] }> {
  const layerCount = await vialGetLayerCount();
  const totalBytes = layerCount * rows * cols * 2;
  const buf = new Uint8Array(totalBytes);
  const chunkMax = 28;
  for (let off = 0; off < totalBytes; off += chunkMax) {
    const sz = Math.min(chunkMax, totalBytes - off);
    const resp = await xfer([ID_DYNAMIC_KEYMAP_GET_BUFFER, (off >> 8) & 0xff, off & 0xff, sz]);
    for (let i = 0; i < sz; i++) {
      buf[off + i] = resp[4 + i] ?? 0;
    }
  }
  const keymaps: number[][][] = [];
  let p = 0;
  for (let l = 0; l < layerCount; l++) {
    const layerRows: number[][] = [];
    for (let r = 0; r < rows; r++) {
      const colsArr: number[] = [];
      for (let c = 0; c < cols; c++) {
        const hi = buf[p++] ?? 0;
        const lo = buf[p++] ?? 0;
        colsArr.push((hi << 8) | lo);
      }
      layerRows.push(colsArr);
    }
    keymaps.push(layerRows);
  }
  return { layerCount, keymaps };
}

export async function vialGetKeycode(layer: number, row: number, col: number): Promise<number> {
  const resp = await xfer([ID_GET_KEYCODE, layer & 0xff, row & 0xff, col & 0xff]);
  return ((resp[4] ?? 0) << 8) | (resp[5] ?? 0);
}

/** Writes a single QMK keycode directly into live RP2040 Vial EEPROM (`< 2 ms`). */
export async function vialSetKeycode(layer: number, row: number, col: number, keycode: number): Promise<void> {
  await xfer([
    ID_SET_KEYCODE,
    layer & 0xff,
    row & 0xff,
    col & 0xff,
    (keycode >> 8) & 0xff,
    keycode & 0xff,
  ]);
}
