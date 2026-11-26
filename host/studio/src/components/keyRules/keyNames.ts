/**
 * Key names and selectors, the way Key Rules and `keys-renderer` read them.
 *
 * A Key Rules row names keys in a comma-separated `keys` string ("F1-F4, Esc, a, home_row,
 * action:mqtt.scene_on, 47"). `keys-custom` splits it into selectors, and the renderer resolves
 * each name to a physical key index. This module mirrors both steps so Studio can (a) show which
 * physical keys a row targets and (b) write a `keys` string for keys picked on the map that the
 * renderer resolves back to exactly those physical keys.
 *
 * The renderer's lookup has two parts. Short names ("A", "ESC", "F2") come from a static table
 * and never move. (keys-custom adds three spellings of its own: `#N` for a physical index, a bare
 * digit for that digit's key, and `comma`.) `KC_*` names are rebound from the live layer-0 keymap (`keyboard/keymap`). The
 * static table doesn't match every physical layout, so when writing, a name is only used if it
 * round-trips to the key that was picked; otherwise the key index is written (`#47`).
 */

/** "NAME=index NAME=index …" (a name may itself be "=" or contain one; the index is after the last). */
function table(src: string): Record<string, number> {
  const out: Record<string, number> = {};
  for (const pair of src.trim().split(/\s+/)) {
    const at = pair.lastIndexOf("=");
    out[pair.slice(0, at)] = Number(pair.slice(at + 1));
  }
  return out;
}

// Copies of the renderer's KEYCODE_TO_INDEX (modules/keys-renderer/script.lua), split into the
// static short names and the `KC_*` names the live keymap rebinds.
const STATIC_SHORT = table(`
  ESC=0 F1=1 F2=2 F3=3 F4=4 F5=5 F6=6 F7=7 F8=8 F9=9 F10=10 F11=11 F12=12 PRT=13 INS=14 DEL=15
  \`=16 1=17 2=18 3=19 4=20 5=21 6=22 7=23 8=24 9=25 0=26 -=27 ==28 BKSP=29 HOME=30
  TAB=31 Q=32 W=33 E=34 R=35 T=36 Y=37 U=38 I=39 O=40 P=41 [=42 ]=43 \\=44 END=45
  CAPS=46 A=47 S=48 D=49 F=50 G=51 H=52 J=53 K=54 L=55 ;=56 '=57 ENTER=58 PGUP=59
  Z=61 X=62 C=63 V=64 B=65 N=66 M=67 ,=68 .=69 /=70 UP=72 PGDN=73
  SPACE=78 LEFT=82 DOWN=83 RIGHT=84
`);

const STATIC_KC = table(`
  KC_ESC=0 KC_F1=1 KC_F2=2 KC_F3=3 KC_F4=4 KC_F5=5 KC_F6=6 KC_F7=7 KC_F8=8 KC_F9=9 KC_F10=10
  KC_F11=11 KC_F12=12 KC_PSCR=13 KC_INS=14 KC_DEL=15
  KC_GRV=16 KC_1=17 KC_2=18 KC_3=19 KC_4=20 KC_5=21 KC_6=22 KC_7=23 KC_8=24 KC_9=25 KC_0=26
  KC_MINS=27 KC_EQL=28 KC_BSPC=29 KC_HOME=30
  KC_TAB=31 KC_Q=32 KC_W=33 KC_E=34 KC_R=35 KC_T=36 KC_Y=37 KC_U=38 KC_I=39 KC_O=40 KC_P=41
  KC_LBRC=42 KC_RBRC=43 KC_BSLS=44 KC_END=45
  KC_CAPS=46 KC_A=47 KC_S=48 KC_D=49 KC_F=50 KC_G=51 KC_H=52 KC_J=53 KC_K=54 KC_L=55
  KC_SCLN=56 KC_QUOT=57 KC_ENT=58 KC_PGUP=59
  KC_LSFT=60 KC_Z=61 KC_X=62 KC_C=63 KC_V=64 KC_B=65 KC_N=66 KC_M=67 KC_COMM=68 KC_DOT=69
  KC_SLSH=70 KC_RSFT=71 KC_UP=72 KC_PGDN=73
  KC_LCTL=74 KC_LALT=75 KC_LGUI=76 KC_SPC=78 KC_RGUI=79 KC_RALT=80 KC_RCTL=81 KC_LEFT=82
  KC_DOWN=83 KC_RGHT=84
`);

// The renderer's KEYCODE_ALIAS (long name -> canonical).
const ALIAS: Record<string, string> = Object.fromEntries(
  `
  KC_ESCAPE=KC_ESC KC_BACKSPACE=KC_BSPC KC_ENTER=KC_ENT KC_RETURN=KC_ENT KC_SPACE=KC_SPC
  KC_MINUS=KC_MINS KC_EQUAL=KC_EQL KC_LBRACKET=KC_LBRC KC_RBRACKET=KC_RBRC KC_BSLASH=KC_BSLS
  KC_SCOLON=KC_SCLN KC_QUOTE=KC_QUOT KC_GRAVE=KC_GRV KC_COMMA=KC_COMM KC_SLASH=KC_SLSH
  KC_DELETE=KC_DEL KC_INSERT=KC_INS KC_PSCREEN=KC_PSCR KC_PGUP=KC_PGUP KC_PAGE_UP=KC_PGUP KC_PGDOWN=KC_PGDN
  KC_PAGE_DOWN=KC_PGDN KC_RIGHT=KC_RGHT KC_LEFT_SHIFT=KC_LSFT KC_RIGHT_SHIFT=KC_RSFT
  KC_LEFT_CTRL=KC_LCTL KC_RIGHT_CTRL=KC_RCTL KC_LEFT_ALT=KC_LALT KC_RIGHT_ALT=KC_RALT
  KC_LEFT_GUI=KC_LGUI KC_RIGHT_GUI=KC_RGUI
`
    .trim()
    .split(/\s+/)
    .map((p) => p.split("=") as [string, string]),
);

/** The renderer tables this module mirrors, for a test that compares them with the Lua. */
export const MIRRORED_TABLES = { keycodeToIndex: { ...STATIC_SHORT, ...STATIC_KC }, alias: ALIAS };

/** Named groups as the renderer defines them (by `KC_*` name, so they follow the live keymap). */
const GROUP_KC: Record<string, string[]> = Object.fromEntries(
  Object.entries({
    fkeys: "F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12",
    numbers: "GRV 1 2 3 4 5 6 7 8 9 0 MINS EQL",
    home_row: "A S D F G H J K L SCLN QUOT",
    arrows: "UP LEFT DOWN RGHT",
    nav: "INS HOME PGUP PGDN END DEL",
    mods: "LSFT RSFT LCTL RCTL LALT RALT LGUI RGUI",
  }).map(([id, names]) => [id, names.split(" ").map((n) => `KC_${n}`)]),
);

export const KEY_GROUPS: Array<{ id: string; label: string; phrase: string }> = [
  { id: "all", label: "All keys", phrase: "all keys" },
  { id: "fkeys", label: "F-keys", phrase: "the F-keys" },
  { id: "numbers", label: "Number row", phrase: "the number row" },
  { id: "letters", label: "Letters", phrase: "letters" },
  { id: "home_row", label: "Home row", phrase: "the home row" },
  { id: "mods", label: "Modifiers", phrase: "modifiers" },
  { id: "arrows", label: "Arrows", phrase: "the arrows" },
  { id: "nav", label: "Nav keys", phrase: "the nav keys" },
];

const GROUP_IDS = new Set(KEY_GROUPS.map((g) => g.id));

export function groupLabel(id: string): string {
  const m = /^row:(\d+)$/.exec(id);
  if (m) return `row ${m[1]}`;
  return KEY_GROUPS.find((g) => g.id === id)?.phrase ?? id;
}

export interface KeyContext {
  /** Physical keys from `keyboard/geometry` (index and row). */
  keys: Array<{ index: number; row?: number }>;
  /** `keyboard/keymap` payload, if retained; its layer 0 rebinds `KC_*` names. */
  keymap?: unknown;
  /** Keycode per physical key on the active layer (`keyboard/resolved_keys`). */
  resolvedKeycodes?: number[];
  /** Action id -> keycode (`system/actions`). */
  actionKeycodes?: Record<string, number>;
}

export interface KeyResolver {
  /** Physical key index for one name, as the renderer resolves it. */
  resolve(name: string | number): number | undefined;
  /** Physical keys a group targets. */
  group(id: string): number[];
  /** Physical keys an action is bound to on the active layer. */
  action(id: string): number[];
  /** A name for this key that resolves back to it (falls back to `#index`). */
  tokenFor(index: number): string;
}

export function buildKeyResolver(ctx: KeyContext): KeyResolver {
  // One table, like the renderer's: short and KC_ names side by side, live layer 0 on top.
  const table: Record<string, number> = { ...STATIC_SHORT, ...STATIC_KC };
  const kcAt = new Map<number, string>();
  const layers = (ctx.keymap as { layers?: Array<{ keyActions?: Array<{ keyCode?: unknown }> }> } | undefined)?.layers;
  const acts = Array.isArray(layers) ? layers[0]?.keyActions : undefined;
  if (Array.isArray(acts)) {
    acts.forEach((a, i) => {
      const code = typeof a?.keyCode === "string" ? a.keyCode.toUpperCase() : "";
      if (!code) return;
      table[code] = i;
      const canon = ALIAS[code];
      if (canon) table[canon] = i;
      if (!kcAt.has(i)) kcAt.set(i, canon ?? code);
    });
  }

  const resolve = (name: string | number): number | undefined => {
    if (typeof name === "number") return Math.floor(name);
    const t = name.trim();
    if (/^-?\d+(\.\d+)?$/.test(t)) return Math.floor(Number(t));
    const up = t.toUpperCase();
    const v = table[up] ?? table[ALIAS[up] ?? ""];
    return typeof v === "number" ? v : undefined;
  };

  const group = (id: string): number[] => {
    const rowM = /^row:(\d+)$/.exec(id);
    if (id === "all") return ctx.keys.map((k) => k.index);
    if (rowM) return ctx.keys.filter((k) => k.row === Number(rowM[1])).map((k) => k.index);
    const names =
      id === "letters"
        ? Array.from({ length: 26 }, (_, i) => `KC_${String.fromCharCode(65 + i)}`)
        : (GROUP_KC[id] ?? []);
    return names.map((n) => resolve(n)).filter((v): v is number => v !== undefined);
  };

  const action = (id: string): number[] => {
    const code = ctx.actionKeycodes?.[id];
    if (code === undefined || !ctx.resolvedKeycodes) return [];
    const out: number[] = [];
    ctx.resolvedKeycodes.forEach((c, i) => {
      if (c === code) out.push(i);
    });
    return out;
  };

  // A static short name is only trusted while the key is bound to what the static table thinks
  // it is ("PGUP" on a key whose layer-0 binding is End would mislead whoever reads the rule).
  const staticAgrees = (index: number) => {
    const partners = Object.entries(STATIC_KC)
      .filter(([, v]) => v === index)
      .map(([n]) => n);
    const live = kcAt.get(index);
    if (live) return partners.includes(live);
    return partners.some((n) => resolve(n) === index);
  };

  const tokenFor = (index: number): string => {
    for (const [name, v] of Object.entries(STATIC_SHORT)) {
      if (v !== index || name === ",") continue; // a comma separates names
      if (tokenTarget(name, resolve) === index && staticAgrees(index)) return prettyName(name);
    }
    if (tokenTarget("comma", resolve) === index) return "comma";
    const live = kcAt.get(index);
    if (live && tokenTarget(live, resolve) === index) return live;
    return `#${index}`;
  };

  return { resolve, group, action, tokenFor };
}

/**
 * One plain key token as `keys-custom` reads it: `#N` is a physical index, a bare digit is that
 * digit's key, `comma` is the comma key, anything else is a name the renderer resolves.
 */
function tokenTarget(tok: string, resolve: (name: string) => number | undefined): number | undefined {
  if (/^#\d+$/.test(tok)) return Number(tok.slice(1));
  if (/^\d$/.test(tok)) return resolve(`KC_${tok}`);
  if (tok.toLowerCase() === "comma") return resolve("KC_COMM");
  return resolve(tok.toUpperCase().startsWith("KC_") ? tok.toUpperCase() : tok);
}

function prettyName(name: string): string {
  if (name.length === 1 || /^F\d+$/.test(name)) return name;
  return name.charAt(0) + name.slice(1).toLowerCase();
}

export interface ParsedKeys {
  /** Every physical key the spec targets, in first-seen order. */
  indices: number[];
  groups: string[];
  actions: string[];
  /** Names the renderer can't resolve (they target nothing). */
  unknown: string[];
}

/** Reads a row's `keys` string the way `keys-custom` and the renderer do. */
export function parseKeySpec(spec: unknown, r: KeyResolver): ParsedKeys {
  const out: ParsedKeys = { indices: [], groups: [], actions: [], unknown: [] };
  const seen = new Set<number>();
  const add = (i: number | undefined) => {
    if (i === undefined || seen.has(i)) return;
    seen.add(i);
    out.indices.push(i);
  };
  for (const raw of String(spec ?? "").split(",")) {
    const tok = raw.trim();
    if (!tok) continue;
    const low = tok.toLowerCase();
    const range = /^f(\d+)\s*-\s*f(\d+)$/.exec(low);
    if (range) {
      for (let n = Number(range[1]); n <= Number(range[2]); n++) add(r.resolve(`KC_F${n}`));
    } else if (GROUP_IDS.has(low) || /^row:\d+$/.test(low)) {
      out.groups.push(low);
      r.group(low).forEach(add);
    } else if (low.startsWith("action:")) {
      const id = tok.slice(7).trim();
      out.actions.push(id);
      r.action(id).forEach(add);
    } else {
      const i = tokenTarget(tok, r.resolve);
      if (i === undefined) out.unknown.push(tok);
      add(i);
    }
  }
  return out;
}

/**
 * A `keys` string for keys picked on the map. A whole group is written by name, F-key runs as
 * ranges ("F1-F4"), everything else by a name that resolves back to the same physical key.
 */
export function formatKeySpec(indices: Iterable<number>, r: KeyResolver, group?: string | null): string {
  if (group) return group;
  const sorted = [...new Set(indices)].sort((a, b) => a - b);
  const parts: string[] = [];
  for (let i = 0; i < sorted.length; i++) {
    const fn = fkeyNumber(sorted[i], r);
    if (fn !== null) {
      let j = i;
      while (j + 1 < sorted.length && fkeyNumber(sorted[j + 1], r) === fn + (j + 1 - i)) j++;
      if (j - i >= 2) {
        parts.push(`F${fn}-F${fn + (j - i)}`);
        i = j;
        continue;
      }
    }
    parts.push(r.tokenFor(sorted[i]));
  }
  return parts.join(", ");
}

function fkeyNumber(index: number, r: KeyResolver): number | null {
  for (let n = 1; n <= 24; n++) if (r.resolve(`KC_F${n}`) === index) return n;
  return null;
}
