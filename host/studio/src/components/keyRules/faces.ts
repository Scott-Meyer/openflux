/**
 * A rough, Studio-side version of `keys-renderer`'s face resolution, for the Keycaps map.
 *
 * It folds every provider's rules per key the way the renderer does (priority, then provider
 * topic, then position; replace / patch / badge / hide; `false` clears). Conditions Studio can
 * see (layer, modifiers) are evaluated; the ones it can't (app, held keys, bus values, time,
 * idle) count as not holding, unless the rule is the one being previewed. For the exact answer
 * on one key, ask the renderer (`keys/inspect/request`).
 */

import type { KeyResolver } from "./keyNames";
import type { RendererRule } from "./rules";

export interface CornerItem {
  text?: string;
  icon?: string;
  dot?: boolean | string;
  color?: string;
}

export interface FxSpec {
  color?: string;
  pulse?: boolean;
  strength?: number;
  shape?: string;
}

export interface Face {
  blank?: boolean;
  label?: string;
  shifted?: string;
  icon?: string;
  image?: string;
  color?: string;
  glow?: FxSpec;
  outline?: FxSpec;
  backdrop?: FxSpec;
  corners?: Partial<Record<"tl" | "tr" | "bl" | "br", CornerItem>>;
}

export interface ProviderPayload {
  topic: string;
  payload: unknown;
}

export interface ResolveContext {
  resolver: KeyResolver;
  activeLayer: number;
  /** Treat this provider topic's rules as replaced by these (Studio's unsaved Key Rules). */
  overrideTopic?: { topic: string; priority: number; mode: string; rules: RendererRule[] };
  /** Rules with this `source` are previewed as if their conditions hold. */
  previewSource?: string | null;
}

const META = new Set([
  "key",
  "keys",
  "group",
  "action",
  "mode",
  "priority",
  "when",
  "mods",
  "corner",
  "hide",
  "sub",
  "toggled",
  "source",
  "name",
  "corners",
]);
const LEGEND = ["label", "shifted", "icon", "image", "shift_case"];

function targets(rule: RendererRule, r: KeyResolver): number[] {
  if (rule.group !== undefined) return r.group(String(rule.group));
  if (rule.action !== undefined) return r.action(String(rule.action));
  if (Array.isArray(rule.keys)) {
    return rule.keys.map((k) => r.resolve(k as string | number)).filter((v): v is number => v !== undefined);
  }
  if (typeof rule.key === "number" || typeof rule.key === "string") {
    const v = r.resolve(rule.key);
    return v === undefined ? [] : [v];
  }
  return [];
}

/** Whether a `when` holds, as far as Studio can tell without the keyboard's live state. */
function whenHolds(when: unknown, ctx: ResolveContext): boolean {
  if (!when || typeof when !== "object") return true;
  const w = when as Record<string, unknown>;
  if (w.layer !== undefined && Number(w.layer) !== ctx.activeLayer) return false;
  if (w.held !== undefined && w.held !== false) return false; // nothing is held in Studio
  if (w.fn !== undefined && w.fn !== false) return false;
  if (w.caps !== undefined && w.caps !== false) return false;
  if (w.host !== undefined && w.host !== true) return false;
  for (const k of ["app", "profile", "bus", "time", "idle_ms"]) {
    if (w[k] !== undefined && w[k] !== "") return false;
  }
  return true;
}

function fx(v: unknown, part: string): FxSpec | undefined {
  if (v === undefined || v === false || v === null) return undefined;
  const base: FxSpec = {
    strength: part === "backdrop" ? 0.55 : 1,
    shape: part === "outline" ? "ring" : part === "backdrop" ? "window" : undefined,
  };
  if (typeof v === "number") return { ...base, strength: v };
  if (typeof v === "string") return { ...base, color: v };
  if (typeof v === "object") {
    const o = v as Record<string, unknown>;
    return {
      ...base,
      color: typeof o.color === "string" ? o.color : undefined,
      pulse: o.pulse === true,
      strength: typeof o.strength === "number" ? o.strength : typeof o.alpha === "number" ? o.alpha : base.strength,
      shape: typeof o.shape === "string" ? o.shape : base.shape,
    };
  }
  return base;
}

function normalizedCorners(rule: RendererRule): Record<string, unknown> | false | undefined {
  let c: Record<string, unknown> | false | undefined;
  if (rule.corners && typeof rule.corners === "object") c = { ...(rule.corners as Record<string, unknown>) };
  else if (rule.corners === false) c = false;
  if (rule.sub !== undefined || rule.toggled !== undefined) {
    if (!c) c = {};
    if (c.tr === undefined) {
      if (rule.sub === false) c.tr = false;
      else if (typeof rule.sub === "string" && rule.sub) c.tr = { text: rule.sub };
      else if (rule.toggled !== undefined) c.tr = { dot: rule.toggled === true };
    }
  }
  return c;
}

function cornerOf(item: unknown): CornerItem | undefined {
  if (item === undefined || item === false || item === null) return undefined;
  if (typeof item !== "object") return { text: String(item) };
  return item as CornerItem;
}

export function applyRule(face: Face | null, rule: RendererRule, mode: string): Face {
  if (mode === "hide" || rule.hide === "all") return { blank: true };
  if (mode === "badge") {
    const f: Face = { ...(face ?? {}) };
    const slot = rule.corner === "tl" || rule.corner === "bl" || rule.corner === "br" ? rule.corner : "tr";
    f.corners = {
      ...(f.corners ?? {}),
      [slot]: { dot: true, color: typeof rule.color === "string" ? rule.color : "#59ebff" },
    };
    return f;
  }
  const f: Record<string, unknown> = mode === "replace" || !face || face.blank ? {} : { ...face };
  if (rule.hide === "legend" || rule.hide === true) for (const k of LEGEND) delete f[k];
  for (const [k, v] of Object.entries(rule)) {
    if (META.has(k)) continue;
    if (v === false) {
      delete f[k];
    } else if (v !== undefined) {
      f[k] = k === "glow" || k === "outline" || k === "backdrop" ? fx(v, k) : v;
      if (k === "icon") delete f.image;
      else if (k === "image") delete f.icon;
    }
  }
  const c = normalizedCorners(rule);
  if (c === false) {
    delete f.corners;
  } else if (c) {
    const merged: Record<string, CornerItem> =
      mode === "replace" ? {} : { ...((f.corners as Record<string, CornerItem>) ?? {}) };
    for (const [slot, item] of Object.entries(c)) {
      const ci = cornerOf(item);
      if (ci) merged[slot] = ci;
      else delete merged[slot];
    }
    f.corners = merged;
  }
  return f as Face;
}

interface Match {
  prio: number;
  ord: number;
  mode: string;
  rule: RendererRule;
}

/** Faces for every key that some rule targets. */
export function resolveFaces(providers: ProviderPayload[], ctx: ResolveContext): Map<number, Face> {
  const list = providers.filter((p) => p.topic !== ctx.overrideTopic?.topic);
  if (ctx.overrideTopic && ctx.overrideTopic.rules.length > 0) {
    list.push({
      topic: ctx.overrideTopic.topic,
      payload: { priority: ctx.overrideTopic.priority, mode: ctx.overrideTopic.mode, rules: ctx.overrideTopic.rules },
    });
  }
  list.sort((a, b) => (a.topic < b.topic ? -1 : a.topic > b.topic ? 1 : 0));
  const perKey = new Map<number, Match[]>();
  let ord = 0;
  for (const { payload } of list) {
    const p = payload as { priority?: number; mode?: string; when?: unknown; rules?: unknown } | null;
    if (!p || !Array.isArray(p.rules)) continue;
    const provOk = whenHolds(p.when, ctx);
    const basePrio = typeof p.priority === "number" ? p.priority : 10;
    const baseMode = typeof p.mode === "string" ? p.mode : "replace";
    for (const raw of p.rules) {
      if (!raw || typeof raw !== "object") continue;
      ord++;
      const rule = raw as RendererRule;
      if (rule.mods !== undefined && rule.mods !== null && Number(rule.mods) !== 0) continue;
      const previewing = ctx.previewSource != null && rule.source === ctx.previewSource;
      if (!previewing && (!provOk || !whenHolds(rule.when, ctx))) continue;
      const m: Match = {
        prio: typeof rule.priority === "number" ? rule.priority : basePrio,
        ord,
        mode: typeof rule.mode === "string" ? rule.mode : baseMode,
        rule,
      };
      for (const ki of targets(rule, ctx.resolver)) {
        const arr = perKey.get(ki) ?? [];
        arr.push(m);
        perKey.set(ki, arr);
      }
    }
  }
  const faces = new Map<number, Face>();
  for (const [ki, ms] of perKey) {
    ms.sort((a, b) => a.prio - b.prio || a.ord - b.ord);
    let face: Face | null = null;
    for (const m of ms) face = applyRule(face, m.rule, m.mode);
    if (face) faces.set(ki, face);
  }
  return faces;
}
