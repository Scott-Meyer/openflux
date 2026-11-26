/**
 * Key Rules rows: the `rules` setting of the `keys-custom` module, the same data
 * `fluxctl keys rules …` and `fluxctl modules config keys-custom --set rules='[...]'` edit.
 *
 * Rows are plain objects with friendly fields (see modules/keys-custom/module.toml). Studio only
 * ever spreads and patches them, so fields it doesn't know about survive. This module reads and
 * writes those fields per *part* (inherit / set / clear), describes a row in a sentence, and
 * translates a row into the renderer rules `keys-custom` would publish (for Studio's preview).
 */

import type { KeyResolver } from "./keyNames";
import { groupLabel, parseKeySpec } from "./keyNames";

export type RuleRow = Record<string, unknown>;

export const CORNERS = ["tl", "tr", "bl", "br"] as const;
export type Corner = (typeof CORNERS)[number];
export const CORNER_NAMES: Record<Corner, string> = {
  tl: "top-left",
  tr: "top-right",
  bl: "bottom-left",
  br: "bottom-right",
};

/** Parts of a key's face a rule can inherit, set or clear. */
export type Part = "label" | "symbol" | "color" | "glow" | "outline" | "backdrop" | `corner_${Corner}`;
export type PartState = "inherit" | "set" | "clear";

export const PARTS: Array<{ id: Part; label: string; hint: string }> = [
  { id: "label", label: "Label", hint: "The text in the middle of the key" },
  { id: "symbol", label: "Icon or image", hint: "A built-in icon or a PNG instead of text" },
  { id: "color", label: "Legend color", hint: "Color of the label and icon" },
  { id: "glow", label: "Glow", hint: "A soft light behind the legend, centered" },
  { id: "outline", label: "Outline", hint: "A ring or window inset from the keycap edge" },
  { id: "backdrop", label: "Backdrop", hint: "An inset tint behind the legend" },
  { id: "corner_tl", label: "Top-left corner", hint: "Text, a dot or an icon, inset from the corner" },
  { id: "corner_tr", label: "Top-right corner", hint: "Where layer-tap and modifier markers live" },
  { id: "corner_bl", label: "Bottom-left corner", hint: "Text, a dot or an icon, inset from the corner" },
  { id: "corner_br", label: "Bottom-right corner", hint: "Text, a dot or an icon, inset from the corner" },
];

/** Row fields a part owns, and the `clear` names that remove it. */
const PART_FIELDS: Record<Part, { fields: string[]; clear: string[] }> = {
  label: { fields: ["label"], clear: ["label"] },
  symbol: { fields: ["icon", "image"], clear: ["icon", "image"] },
  color: { fields: ["color"], clear: ["color"] },
  glow: { fields: ["glow", "glow_color"], clear: ["glow"] },
  outline: { fields: ["outline", "outline_color", "outline_pulse"], clear: ["outline"] },
  backdrop: { fields: ["backdrop"], clear: ["backdrop"] },
  corner_tl: { fields: ["corner_tl"], clear: [] },
  corner_tr: { fields: ["corner_tr"], clear: [] },
  corner_bl: { fields: ["corner_bl"], clear: [] },
  corner_br: { fields: ["corner_br"], clear: [] },
};

/** Values a part gets when it's switched to "set" (so a set part is never silently empty). */
const SET_DEFAULTS: Partial<Record<Part, RuleRow>> = {
  glow: { glow: "soft" },
  outline: { outline: "ring" },
  backdrop: { backdrop: "#1e3a5f" },
  color: { color: "#ffd166" },
  symbol: { icon: "star" },
  label: { label: "Label" },
  corner_tl: { corner_tl: "dot" },
  corner_tr: { corner_tr: "dot" },
  corner_bl: { corner_bl: "dot" },
  corner_br: { corner_br: "dot" },
};

export function str(v: unknown): string {
  return typeof v === "string" ? v.trim() : typeof v === "number" ? String(v) : "";
}

export function clearList(row: RuleRow): string[] {
  return str(row.clear)
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function withClear(row: RuleRow, names: string[], on: boolean): RuleRow {
  const cur = clearList(row).filter((n) => !names.includes(n));
  const next = on ? [...cur, ...names] : cur;
  return { ...row, clear: next.join(",") };
}

const EMPTY: Record<string, unknown> = { glow: "none", outline: "none", outline_pulse: false };

function fieldIsSet(row: RuleRow, f: string): boolean {
  if (f === "outline_pulse") return false; // a modifier of `outline`, not a part on its own
  const v = str(row[f]);
  return v !== "" && v !== "none";
}

export function partState(row: RuleRow, part: Part): PartState {
  const spec = PART_FIELDS[part];
  if (part.startsWith("corner_") && str(row[part]) === "none") return "clear";
  const cleared = clearList(row);
  if (spec.clear.some((c) => cleared.includes(c))) return "clear";
  return spec.fields.some((f) => fieldIsSet(row, f)) ? "set" : "inherit";
}

/** The row with `part` switched to a state. A part that's already in that state is left alone. */
export function setPartState(row: RuleRow, part: Part, state: PartState): RuleRow {
  if (partState(row, part) === state) return row;
  const spec = PART_FIELDS[part];
  let next: RuleRow = { ...row };
  for (const f of spec.fields) next[f] = EMPTY[f] ?? "";
  if (spec.clear.length > 0) {
    next = withClear(next, spec.clear, state === "clear");
    if (str(next.clear) === "" && !("clear" in row)) delete next.clear;
  }
  if (state === "clear" && part.startsWith("corner_")) next[part] = "none";
  if (state === "set") next = { ...next, ...SET_DEFAULTS[part] };
  return next;
}

export type LegendMode = "keep" | "legend" | "all";
export function legendMode(row: RuleRow): LegendMode {
  const h = str(row.hide);
  return h === "legend" || h === "all" ? h : "keep";
}

/** A new rule for these keys. */
export function newRule(keys: string, look: RuleRow = {}): RuleRow {
  return {
    name: "",
    enabled: true,
    keys,
    layer: "any",
    app: "",
    held: "any",
    caps: "any",
    hide: "nothing",
    ...look,
  };
}

/** Quick starts for "on these keys, …". */
export const QUICK_LOOKS: Array<{ id: string; label: string; look: RuleRow }> = [
  { id: "hide", label: "Hide legend", look: { hide: "legend" } },
  { id: "glow", label: "Glow", look: { glow: "soft", glow_color: "#ffd166" } },
  { id: "outline", label: "Outline", look: { outline: "ring", outline_color: "#4de1ff" } },
  { id: "label", label: "Relabel", look: { label: "Label" } },
  { id: "corner", label: "Corner text", look: { corner_br: "1" } },
  { id: "backdrop", label: "Backdrop", look: { backdrop: "#1e3a5f" } },
  { id: "blank", label: "Blank key", look: { hide: "all" } },
];

// ---------- Sentences ----------

export interface DescribeContext {
  resolver: KeyResolver;
  /** Display name for a physical key (its current legend, or a fallback). */
  keyName: (index: number) => string;
}

export function describeKeys(spec: unknown, ctx: DescribeContext): string {
  const s = str(spec);
  if (!s) return "no keys yet";
  const p = parseKeySpec(s, ctx.resolver);
  const bits: string[] = [];
  for (const g of p.groups) bits.push(groupLabel(g));
  for (const a of p.actions) bits.push(`wherever ${a} is bound`);
  const fromGroups = new Set(p.groups.flatMap((g) => ctx.resolver.group(g)));
  const fromActions = new Set(p.actions.flatMap((a) => ctx.resolver.action(a)));
  const loose = p.indices.filter((i) => !fromGroups.has(i) && !fromActions.has(i));
  const fRun = /^\s*f(\d+)\s*-\s*f(\d+)\s*$/i;
  const ranges = s.split(",").filter((t) => fRun.test(t));
  if (ranges.length > 0 && loose.length > 6) {
    bits.push(...ranges.map((t) => t.trim().toUpperCase().replace("-", "–")));
  } else if (loose.length > 8) {
    bits.push(`${loose.length} keys`);
  } else if (loose.length > 0) {
    bits.push(...loose.map(ctx.keyName));
  }
  for (const u of p.unknown) bits.push(`"${u}" (unknown key)`);
  return joinAnd(bits);
}

const OP_WORDS: Record<string, string> = {
  eq: "is",
  ne: "is not",
  in: "is one of",
  gt: "is above",
  lt: "is below",
  contains: "contains",
  exists: "has a value",
  missing: "has no value",
};

export const BUS_OPS = Object.keys(OP_WORDS);
export function opWords(op: string): string {
  return OP_WORDS[op] ?? op;
}

export function describeConditions(row: RuleRow): string[] {
  const out: string[] = [];
  const layer = str(row.layer);
  if (layer && layer !== "any") out.push(`on layer ${layer}`);
  const app = str(row.app);
  if (app)
    out.push(
      `in ${joinOr(
        app
          .split(",")
          .map((a) => a.trim())
          .filter(Boolean),
      )}`,
    );
  if (str(row.held) === "held") out.push("while the key is held");
  if (str(row.held) === "not_held") out.push("while the key isn't held");
  if (str(row.caps) === "on") out.push("while Caps Lock is on");
  if (str(row.caps) === "off") out.push("while Caps Lock is off");
  const time = str(row.time);
  if (time) out.push(`between ${time.replace("-", " and ")}`);
  const idle = Number(row.idle_seconds);
  if (Number.isFinite(idle) && idle > 0) out.push(`after ${formatSeconds(idle)} idle`);
  const topic = str(row.condition_topic);
  if (topic) {
    const op = str(row.condition_op) || "eq";
    const path = str(row.condition_path);
    const subject = path ? `${topic} → ${path}` : topic;
    const val = op === "exists" || op === "missing" ? "" : ` "${str(row.condition_value)}"`;
    out.push(`when ${subject} ${opWords(op)}${val}`);
  }
  return out;
}

function formatSeconds(s: number): string {
  if (s >= 3600 && s % 3600 === 0) return `${s / 3600} h`;
  if (s >= 60 && s % 60 === 0) return `${s / 60} min`;
  return `${s} s`;
}

function cornerWords(v: string): string {
  if (v === "dot" || v === "•") return "a dot";
  if (v.startsWith("icon:")) return `the ${v.slice(5).trim()} icon`;
  return `"${v}"`;
}

export function describeLook(row: RuleRow): string[] {
  const out: string[] = [];
  const mode = legendMode(row);
  if (mode === "all") return ["blank the key"];
  if (mode === "legend") out.push("hide the legend (markers stay)");
  const cleared = clearList(row);
  const label = str(row.label);
  if (label) out.push(`show "${label}"`);
  if (str(row.icon)) out.push(`show the ${str(row.icon)} icon`);
  else if (str(row.image)) out.push(`show ${str(row.image).split("/").pop()}`);
  if (str(row.color)) out.push(`color the legend ${str(row.color)}`);
  const glow = str(row.glow);
  if (glow && glow !== "none") {
    const how = glow === "pulse" ? "pulse a glow" : glow === "subtle" ? "glow subtly" : "glow";
    out.push(str(row.glow_color) ? `${how} in ${str(row.glow_color)}` : how);
  }
  const outline = str(row.outline);
  if (outline && outline !== "none") {
    const shape = outline === "window" ? "window outline" : outline === "edge" ? "edge outline" : "ring";
    const color = str(row.outline_color) ? ` in ${str(row.outline_color)}` : "";
    out.push(`draw a ${row.outline_pulse === true ? "pulsing " : ""}${shape}${color}`);
  }
  if (str(row.backdrop)) out.push(`tint the backdrop ${str(row.backdrop)}`);
  const clearedCorners: string[] = [];
  for (const c of CORNERS) {
    const v = str(row[`corner_${c}`]);
    if (v === "none") clearedCorners.push(CORNER_NAMES[c]);
    else if (v) out.push(`put ${cornerWords(v)} in the ${CORNER_NAMES[c]} corner`);
  }
  const clearNames = cleared.map((c) => (c === "icon" || c === "image" ? "icon" : c === "color" ? "legend color" : c));
  const uniqueClears = [...new Set(clearNames)];
  if (clearedCorners.length > 0)
    uniqueClears.push(`${joinAnd(clearedCorners)} corner${clearedCorners.length > 1 ? "s" : ""}`);
  if (uniqueClears.length > 0) out.push(`remove the ${joinAnd(uniqueClears)}`);
  return out;
}

/** "On F1–F4, on layer 0: hide the legend (markers stay) and glow." */
export function describeRule(row: RuleRow, ctx: DescribeContext): string {
  const where = [`On ${describeKeys(row.keys, ctx)}`, ...describeConditions(row)].join(", ");
  const look = describeLook(row);
  return `${where}: ${look.length > 0 ? joinAnd(look) : "nothing yet (pick a look)"}.`;
}

export function joinAnd(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

function joinOr(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} or ${items[items.length - 1]}`;
}

// ---------- Renderer rules (preview mirror of modules/keys-custom/script.lua) ----------

export type RendererRule = Record<string, unknown>;

const CLEARABLE = new Set(["label", "shifted", "icon", "image", "color", "glow", "outline", "backdrop", "corners"]);

function cornerItem(v: string): unknown {
  if (!v) return undefined;
  if (v === "dot" || v === "•") return { dot: true };
  if (v.startsWith("icon:")) return { icon: v.slice(5).trim() };
  return { text: v };
}

/** The look (`set`) and conditions (`when`) of a row, as `keys-custom` builds them. */
export function rowToParts(row: RuleRow): { set: RendererRule; when?: RendererRule } {
  const set: RendererRule = {};
  const when: RendererRule = {};
  const hide = str(row.hide);
  if (hide === "legend" || hide === "all") set.hide = hide;
  if (str(row.label)) set.label = row.label;
  if (str(row.icon)) set.icon = row.icon;
  if (str(row.image)) set.image = row.image;
  if (str(row.color)) set.color = row.color;
  const glow = str(row.glow);
  if (glow && glow !== "none") {
    set.glow = {
      color: str(row.glow_color) || undefined,
      pulse: glow === "pulse",
      strength: glow === "subtle" ? 0.5 : 1.0,
    };
  }
  const outline = str(row.outline);
  if (outline && outline !== "none") {
    set.outline = { shape: outline, color: str(row.outline_color) || undefined, pulse: row.outline_pulse === true };
  }
  if (str(row.backdrop)) set.backdrop = { color: row.backdrop, alpha: 0.45 };
  const corners: Record<string, unknown> = {};
  for (const c of CORNERS) {
    const v = str(row[`corner_${c}`]);
    if (v === "none") corners[c] = false;
    else if (v) corners[c] = cornerItem(v);
  }
  if (Object.keys(corners).length > 0) set.corners = corners;
  for (const part of clearList(row)) if (CLEARABLE.has(part)) set[part] = false;

  const layer = str(row.layer);
  if (layer && layer !== "any" && Number.isFinite(Number(layer))) when.layer = Number(layer);
  if (str(row.app)) when.app = row.app;
  if (str(row.held) === "held") when.held = true;
  else if (str(row.held) === "not_held") when.held = false;
  if (str(row.caps) === "on") when.caps = true;
  else if (str(row.caps) === "off") when.caps = false;
  if (str(row.time)) when.time = row.time;
  const idle = Number(row.idle_seconds);
  if (Number.isFinite(idle) && idle > 0) when.idle_ms = Math.floor(idle * 1000);
  const topic = str(row.condition_topic);
  if (topic) {
    const op = str(row.condition_op) || "eq";
    const c: Record<string, unknown> = { topic, path: str(row.condition_path) || undefined };
    if (op === "exists") c.exists = true;
    else if (op === "missing") c.exists = false;
    else if (op === "in")
      c.in = str(row.condition_value)
        .split(",")
        .map((v) => v.trim())
        .filter(Boolean);
    else c[op] = row.condition_value;
    when.bus = c;
  }
  return { set, when: Object.keys(when).length > 0 ? when : undefined };
}

/** The physical keys a row targets (the preview applies a row per key, like the renderer). */
function rowSelectors(spec: unknown, r: KeyResolver): RendererRule[] {
  const keys = parseKeySpec(spec, r).indices;
  return keys.length > 0 ? [{ keys }] : [];
}

export interface CustomOverride {
  label?: string;
  color?: string;
  bg?: string;
  border?: string;
  icon?: string;
  image?: string;
  hide?: boolean;
}

function legacySet(ov: CustomOverride | undefined): RendererRule | null {
  if (!ov || typeof ov !== "object") return null;
  const set: RendererRule = {};
  if (str(ov.label)) set.label = ov.label;
  if (str(ov.color)) set.color = ov.color;
  if (str(ov.icon)) set.icon = ov.icon;
  if (str(ov.image)) set.image = ov.image;
  if (str(ov.bg)) set.backdrop = { color: ov.bg, alpha: 0.45 };
  if (str(ov.border)) set.outline = { shape: "window", color: ov.border };
  if (ov.hide === true) set.hide = "legend";
  return Object.keys(set).length > 0 ? set : null;
}

/**
 * The `keys/providers/custom` rules this config publishes (overrides, then rows in order).
 * Studio previews with this so edits show before the module reloads.
 */
export function customProviderRules(
  cfg: { rules?: unknown; by_key?: Record<string, CustomOverride>; by_action?: Record<string, CustomOverride> },
  r: KeyResolver,
): RendererRule[] {
  const out: RendererRule[] = [];
  for (const [actionId, ov] of Object.entries(cfg.by_action ?? {})) {
    const set = legacySet(ov);
    if (set) out.push({ priority: 100, action: actionId, source: `by_action:${actionId}`, ...set });
  }
  for (const [k, ov] of Object.entries(cfg.by_key ?? {})) {
    const m = /^([^:]+):(\d+)$/.exec(k);
    const set = legacySet(ov);
    if (!m || !set) continue;
    const when = m[1] !== "*" && Number.isFinite(Number(m[1])) ? { layer: Number(m[1]) } : undefined;
    out.push({ priority: 101, key: Number(m[2]), when, source: `by_key:${k}`, ...set });
  }
  const rows = Array.isArray(cfg.rules) ? cfg.rules : [];
  rows.forEach((row, i) => {
    if (!row || typeof row !== "object" || (row as RuleRow).enabled === false) return;
    const { set, when } = rowToParts(row as RuleRow);
    if (Object.keys(set).length === 0) return;
    for (const sel of rowSelectors((row as RuleRow).keys, r)) {
      out.push({ priority: 110 + i + 1, when, source: `rules:${i + 1}`, ...sel, ...set });
    }
  });
  return out;
}
