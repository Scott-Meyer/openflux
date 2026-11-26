/**
 * "Why does this key look like this?": ask `keys-renderer` over the bus and put its answer
 * into words.
 *
 * Request: `keys/inspect/request` `{ key, request_id }`. Answer: `keys/inspect/result` with
 * every rule that targets the key in precedence order (low to high), whether each matched, and
 * why not; plus the resolved face and any active signal.
 */

import { v2Client } from "../../api/v2Client";
import type { RuleRow } from "./rules";
import { CORNER_NAMES, describeConditions, joinAnd, rowToParts, str } from "./rules";

export interface InspectStep {
  provider: string;
  rule?: number;
  priority?: number;
  mode?: string;
  matched?: boolean;
  skipped?: string;
  source?: string;
  name?: string;
}

export interface InspectResult {
  key: number;
  request_id?: string;
  layer?: number;
  mods?: number;
  steps: InspectStep[];
  face?: Record<string, unknown>;
  signal?: { until_ms?: number } | null;
}

type Bus = Pick<typeof v2Client, "getStatus" | "publishBus" | "pollBusEvents">;

/** Asks the renderer to explain one key. Rejects if nothing answers within `timeoutMs`. */
export async function inspectKey(key: number, timeoutMs = 3000, bus: Bus = v2Client): Promise<InspectResult> {
  const requestId = `studio-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  let since = (await bus.getStatus()).latest_bus_seq ?? 0;
  await bus.publishBus("keys/inspect/request", { key, request_id: requestId });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const wait = Math.max(50, Math.min(1000, deadline - Date.now()));
    const res = await bus.pollBusEvents("keys/inspect/result", since, wait);
    for (const ev of res.events) {
      const p = ev.payload as InspectResult | null;
      if (p && p.request_id === requestId) return { ...p, steps: Array.isArray(p.steps) ? p.steps : [] };
    }
    since = Math.max(since, res.latest_seq ?? since);
  }
  throw new Error("The key renderer didn't answer. Is keys-renderer enabled and running?");
}

export interface ExplainContext {
  /** Retained `keys/providers/<id>` payloads by topic. */
  providers: Record<string, unknown>;
  /** The user's Key Rules rows (for naming "your rule 3"). */
  rows: RuleRow[];
  activeLayer: number;
}

export interface ExplainedStep {
  /** Unique within one answer: provider topic and rule position. */
  id: string;
  /** Who: "keymap", "Your rule 2 “Hide legends”". */
  who: string;
  /** What it does: "sets the face to “A”", "hides the legend". */
  what: string;
  /** Why it didn't apply, if it didn't. */
  skipped?: string;
  priority?: number;
  mode?: string;
  matched: boolean;
  /** `rules:<n>` when the step is one of the user's rows. */
  source?: string;
  /** 0-based row index when the step is one of the user's rows. */
  rowIndex?: number;
}

const MODULE_NAMES: Record<string, string> = {
  keymap: "Keymap legends",
  "keymap-bindings": "Binding markers (layer-tap, mod-tap)",
  shortcuts: "Shortcut legends",
  custom: "Key Rules",
};

function providerName(topic: string): string {
  const id = topic.replace(/^keys\/providers\//, "");
  return MODULE_NAMES[id] ?? id;
}

/** Which of the user's things a `keys/providers/custom` step comes from. */
function customSource(step: InspectStep): { source?: string; row?: number } {
  if (step.source) {
    const m = /^rules:(\d+)$/.exec(step.source);
    return { source: step.source, row: m ? Number(m[1]) - 1 : undefined };
  }
  // Older keys-custom without `source`: the band layout (100 action, 101 key, 110 + row).
  const p = step.priority ?? 0;
  if (p === 100) return { source: "by_action" };
  if (p === 101) return { source: "by_key" };
  if (p > 110) return { source: `rules:${p - 110}`, row: p - 111 };
  return {};
}

function lookup(ctx: ExplainContext, step: InspectStep): Record<string, unknown> | undefined {
  const prov = ctx.providers[step.provider] as { rules?: unknown[] } | undefined;
  if (!prov || !Array.isArray(prov.rules) || typeof step.rule !== "number") return undefined;
  const r = prov.rules[step.rule - 1];
  return r && typeof r === "object" ? (r as Record<string, unknown>) : undefined;
}

function quote(v: unknown): string {
  return `“${String(v)}”`;
}

function cornerText(item: unknown): string {
  if (item === false) return "nothing";
  if (!item || typeof item !== "object") return quote(item);
  const o = item as Record<string, unknown>;
  if (o.dot !== undefined) return "a dot";
  if (o.icon) return `the ${o.icon} icon`;
  return quote(o.text ?? "");
}

/** What a renderer rule does to a face, in words. */
export function describeRendererRule(rule: Record<string, unknown> | undefined, mode: string | undefined): string {
  if (mode === "hide" || rule?.hide === "all") return "blanks the key";
  if (!rule) return mode === "replace" ? "sets the face" : "changes the face";
  if (mode === "badge")
    return `adds a dot in the ${CORNER_NAMES[(rule.corner as keyof typeof CORNER_NAMES) ?? "tr"] ?? "top-right"} corner`;
  const bits: string[] = [];
  if (rule.hide === "legend" || rule.hide === true) bits.push("hides the legend");
  const sets = (k: string) => rule[k] !== undefined && rule[k] !== false;
  const clears: string[] = [];
  for (const k of ["label", "shifted", "icon", "image", "color", "glow", "outline", "backdrop", "corners"]) {
    if (rule[k] === false) clears.push(k);
  }
  if (sets("label") && String(rule.label) !== "") bits.push(`shows ${quote(rule.label)}`);
  if (sets("icon")) bits.push(`shows the ${rule.icon} icon`);
  if (sets("image")) bits.push(`shows ${String(rule.image).split("/").pop()}`);
  if (sets("color")) bits.push(`colors it ${rule.color}`);
  if (sets("glow")) bits.push("glows");
  if (sets("outline")) bits.push("draws an outline");
  if (sets("backdrop")) bits.push("tints the backdrop");
  if (typeof rule.sub === "string" && rule.sub) bits.push(`puts ${quote(rule.sub)} top-right`);
  if (rule.corners && typeof rule.corners === "object") {
    for (const [slot, item] of Object.entries(rule.corners as Record<string, unknown>)) {
      const where = CORNER_NAMES[slot as keyof typeof CORNER_NAMES] ?? slot;
      bits.push(item === false ? `clears the ${where} corner` : `puts ${cornerText(item)} ${where}`);
    }
  }
  if (rule.cd !== undefined) bits.push("shows a cooldown");
  if (rule.charges !== undefined) bits.push("shows charges");
  if (clears.length > 0) bits.push(`removes the ${joinAnd(clears)}`);
  const verb = mode === "replace" ? "sets the face" : "changes the face";
  if (bits.length === 0) return verb;
  return mode === "replace" ? `sets the face: ${joinAnd(bits)}` : joinAnd(bits);
}

const SKIP_WORDS: Record<string, string> = {
  modifiers: "it only applies while certain modifiers are held",
  "provider condition": "its module's condition doesn't hold right now",
  layer: "it's for another layer",
  app: "the app in front doesn't match",
  profile: "the active profile doesn't match",
  fn: "it depends on Fn being held",
  held: "it depends on a key being held",
  caps: "it depends on Caps Lock",
  time: "it's outside its time range",
  idle: "the keyboard hasn't been idle long enough",
  host: "it depends on whether a computer is connected",
  condition: "its condition doesn't hold right now",
};

function skipWords(why: string, rule: Record<string, unknown> | undefined, ctx: ExplainContext): string {
  if (why.startsWith("bus ")) {
    return `${why.slice(4)} doesn't have the value it waits for (or has no value yet)`;
  }
  let base = SKIP_WORDS[why] ?? why;
  const when = rule?.when as Record<string, unknown> | undefined;
  if (why === "layer" && when?.layer !== undefined)
    base = `it's for layer ${when.layer}, and layer ${ctx.activeLayer} is active`;
  if (why === "modifiers" && typeof rule?.mods === "number")
    base = `it only applies while ${modNames(rule.mods)} ${rule.mods & (rule.mods - 1) ? "are" : "is"} held`;
  if (why === "app" && when?.app) base = `it's only for ${when.app}`;
  if (why === "time" && when?.time) base = `it's only between ${String(when.time).replace("-", " and ")}`;
  return base;
}

function modNames(mask: number): string {
  const n: string[] = [];
  if (mask & 1) n.push("Ctrl");
  if (mask & 2) n.push("Shift");
  if (mask & 4) n.push("Alt");
  if (mask & 8) n.push("Cmd");
  return n.join("+") || "no modifier";
}

/** Each step, highest precedence first (the order people read "why"). */
export function explainSteps(result: InspectResult, ctx: ExplainContext): ExplainedStep[] {
  const out: ExplainedStep[] = [];
  for (const step of result.steps) {
    const rule = lookup(ctx, step);
    const isCustom = step.provider === "keys/providers/custom";
    let who = providerName(step.provider);
    let rowIndex: number | undefined;
    let source = step.source;
    if (isCustom) {
      const cs = customSource(step);
      source = cs.source;
      if (cs.row !== undefined) {
        rowIndex = cs.row;
        const row = ctx.rows[cs.row];
        const name = str(step.name) || str(row?.name);
        who = `Your rule ${cs.row + 1}${name ? ` ${quote(name)}` : ""}`;
      } else if (cs.source?.startsWith("by_key")) {
        who = "This key's override";
      } else if (cs.source?.startsWith("by_action")) {
        who = "The override for this action";
      }
    }
    const matched = step.matched !== false && !step.skipped;
    // A user row says the same thing as the rule it became (if the provider isn't retained yet).
    const row = rowIndex !== undefined ? ctx.rows[rowIndex] : undefined;
    const what = describeRendererRule(rule ?? (row ? rowToParts(row).set : undefined), step.mode);
    let skipped: string | undefined;
    if (!matched) {
      skipped = skipWords(step.skipped ?? "condition", rule, ctx);
      if (row && (step.skipped === "condition" || !step.skipped)) {
        const conds = describeConditions(row);
        if (conds.length > 0) skipped = `it only applies ${joinAnd(conds)}`;
      }
    }
    out.push({
      id: `${step.provider}#${step.rule ?? out.length}`,
      who,
      what,
      skipped,
      priority: step.priority,
      mode: step.mode,
      matched,
      source,
      rowIndex,
    });
  }
  return out.reverse();
}

/** The resolved face in one sentence ("Shows “A”, with “L2” top-right and a glow."). */
export function describeFace(face: Record<string, unknown> | undefined): string {
  if (!face || Object.keys(face).length === 0) return "Nothing is drawn on this key.";
  if (face.blank) return "The key is blank.";
  const bits: string[] = [];
  if (face.label) bits.push(`shows ${quote(face.label)}`);
  if (face.icon) bits.push(`shows the ${face.icon} icon`);
  if (face.image) bits.push(`shows ${String(face.image).split("/").pop()}`);
  if (!face.label && !face.icon && !face.image) bits.push("has no legend");
  if (face.corners && typeof face.corners === "object") {
    for (const [slot, item] of Object.entries(face.corners as Record<string, unknown>)) {
      bits.push(`${cornerText(item)} ${CORNER_NAMES[slot as keyof typeof CORNER_NAMES] ?? slot}`);
    }
  }
  if (face.glow) bits.push("a glow");
  if (face.outline) bits.push("an outline");
  if (face.backdrop) bits.push("a backdrop");
  const s = joinAnd(bits);
  return `${s.charAt(0).toUpperCase()}${s.slice(1)}.`;
}

/** Sends a 3 s pulsing outline to these keys, to find them on the keyboard. */
export async function testPulse(keys: number[], bus: Pick<typeof v2Client, "publishBus"> = v2Client): Promise<void> {
  await bus.publishBus("keys/signal", {
    id: "studio-test-pulse",
    keys,
    set: { outline: { pulse: true } },
    duration_ms: 3000,
  });
}
