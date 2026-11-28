import type {
  ActuationMode,
  AppearanceConfig,
  AppearanceTheme,
  KeyAction,
  KeymapConfig,
  KeyglassConfig,
  ShortcutProfile,
} from "../types/keyglass";

/** Used when the daemon doesn't report the keyboard's limits. */
export const DEFAULT_MAX_LAYERS = 8;
export const DEFAULT_MAX_ACTUATION_MODES = 8;

/**
 * The outcome of a layer or actuation-mode edit. An accepted edit carries the config sections it
 * changes, ready for one config patch, and the side effects the user should confirm before it is
 * applied (empty when there are none). A refused edit says why.
 */
export type ConfigEdit =
  | { ok: true; patch: Partial<KeyglassConfig>; consequences: string[]; summary: string }
  | { ok: false; reason: string };

const refuse = (reason: string): ConfigEdit => ({ ok: false, reason });

export const maxLayers = (config: KeyglassConfig) => config.keymapLimits?.maxLayers ?? DEFAULT_MAX_LAYERS;
export const maxActuationModes = (config: KeyglassConfig) =>
  config.keymapLimits?.maxActuationModes ?? DEFAULT_MAX_ACTUATION_MODES;

/** The layer count; layouts are edited together, so they have the same count. */
export const layerCount = (keymap: KeymapConfig) =>
  Math.max(0, ...Object.values(keymap.keymaps).map((l) => l.layers.length));

const layerName = (keymap: KeymapConfig, idx: number) => {
  const lay =
    keymap.keymaps[keymap.selectedFrameLayoutType]?.layers[idx] ?? Object.values(keymap.keymaps)[0]?.layers[idx];
  return lay?.name?.trim() || `Layer ${idx}`;
};

const profileName = (p: ShortcutProfile) => `“${p.name}”`;

const uniqueId = (prefix: string, taken: Iterable<string>) => {
  const used = new Set(taken);
  let n = Date.now().toString(36);
  while (used.has(`${prefix}-${n}`)) n += "x";
  return `${prefix}-${n}`;
};

/** Adds an empty layer (every key falls through) at the top of every layout. */
export function addLayer(config: KeyglassConfig, name: string): ConfigEdit {
  const km = config.keymap;
  const count = layerCount(km);
  const limit = maxLayers(config);
  if (count >= limit) return refuse(`The keyboard holds at most ${limit} layers.`);
  const trimmed = name.trim() || `Layer ${count}`;
  const keymaps: KeymapConfig["keymaps"] = {};
  for (const [lname, ldata] of Object.entries(km.keymaps)) {
    const keys = ldata.layers[0]?.keyActions.length ?? 0;
    const layer = {
      name: trimmed,
      id: uniqueId(`${lname}-layer`, []),
      frameLayoutType: lname,
      keyActions: Array.from({ length: keys }, (): KeyAction => ({ type: 0, keyCode: "KC_TRANSPARENT" })),
    };
    keymaps[lname] = { ...ldata, layers: [...ldata.layers, layer] };
  }
  return {
    ok: true,
    patch: { keymap: { ...km, keymaps } },
    consequences: [],
    summary: `Added layer L${count}: ${trimmed}.`,
  };
}

/** Renames the layer at `idx` in every layout. */
export function renameLayer(config: KeyglassConfig, idx: number, name: string): ConfigEdit {
  const trimmed = name.trim();
  if (!trimmed) return refuse("A layer needs a name.");
  const km = config.keymap;
  if (idx < 0 || idx >= layerCount(km)) return refuse(`There is no layer ${idx}.`);
  const keymaps: KeymapConfig["keymaps"] = {};
  for (const [lname, ldata] of Object.entries(km.keymaps)) {
    keymaps[lname] = { ...ldata, layers: ldata.layers.map((l, i) => (i === idx ? { ...l, name: trimmed } : l)) };
  }
  return {
    ok: true,
    patch: { keymap: { ...km, keymaps } },
    consequences: [],
    summary: `Renamed layer L${idx} to ${trimmed}.`,
  };
}

/** A layer index after `deleted` is removed: the same layer's new index, or `fallback` for the deleted one. */
const shiftAfterDelete = (idx: number, deleted: number, fallback: number) =>
  idx === deleted ? fallback : idx > deleted ? idx - 1 : idx;

/** Theme stylemaps are keyed by layer index; the deleted layer's styles go, later layers move down. */
function themeWithoutLayer(theme: AppearanceTheme, deleted: number): AppearanceTheme {
  const stylemaps: AppearanceTheme["stylemaps"] = {};
  for (const [key, value] of Object.entries(theme.stylemaps ?? {})) {
    const n = Number(key);
    if (!Number.isInteger(n) || String(n) !== key) {
      stylemaps[key] = value;
    } else if (n !== deleted) {
      stylemaps[String(n > deleted ? n - 1 : n)] = value;
    }
  }
  return { ...theme, stylemaps };
}

const themeHasLayerStyles = (theme: AppearanceTheme, idx: number) =>
  Object.keys(theme.stylemaps?.[String(idx)]?.styles ?? {}).length > 0;

/**
 * Deletes the layer at `idx` from every layout. The base layer can't be deleted. Keys that
 * switch to the layer are cleared (to "no key" on the base layer, to fall-through elsewhere),
 * profiles and layouts that start on it start on the base layer, and its key styles go.
 */
export function deleteLayer(config: KeyglassConfig, idx: number): ConfigEdit {
  const km = config.keymap;
  const count = layerCount(km);
  if (idx < 0 || idx >= count) return refuse(`There is no layer ${idx}.`);
  if (idx === 0) return refuse("The base layer can't be deleted: every other layer falls through to it.");
  const name = layerName(km, idx);
  const consequences: string[] = [];

  let switchKeys = 0;
  const keymaps: KeymapConfig["keymaps"] = {};
  let layoutStartsHere = false;
  for (const [lname, ldata] of Object.entries(km.keymaps)) {
    const layers = ldata.layers
      .filter((_, i) => i !== idx)
      .map((lay, layIdx) => ({
        ...lay,
        keyActions: lay.keyActions.map((ka): KeyAction => {
          if (ka.type !== 2) return ka;
          const target = ka.targetLayer ?? 1;
          if (target === idx) {
            if (lname === km.selectedFrameLayoutType) switchKeys += 1;
            return { type: 0, keyCode: layIdx === 0 ? "KC_NO" : "KC_TRANSPARENT" };
          }
          return target > idx ? { ...ka, targetLayer: target - 1 } : ka;
        }),
      }));
    if (ldata.defaultLayer === idx) layoutStartsHere = true;
    keymaps[lname] = { ...ldata, defaultLayer: shiftAfterDelete(ldata.defaultLayer ?? 0, idx, 0), layers };
  }
  if (switchKeys > 0) {
    consequences.push(
      `${switchKeys} key${switchKeys === 1 ? "" : "s"} that switch${switchKeys === 1 ? "es" : ""} to ${name} will be cleared.`,
    );
  }
  if (layoutStartsHere) consequences.push(`The keyboard starts on ${name}; it will start on the base layer.`);

  const profiles = config.profiles?.map((p) => ({
    ...p,
    default_layer: shiftAfterDelete(p.default_layer ?? 0, idx, 0),
  }));
  for (const p of config.profiles ?? []) {
    if ((p.default_layer ?? 0) === idx) {
      consequences.push(`Profile ${profileName(p)} starts on ${name}; it will start on the base layer.`);
    }
  }

  const themes = [config.appearance.defaultTheme, ...(config.appearance.customThemes ?? [])];
  const styled = themes.filter((t) => themeHasLayerStyles(t, idx));
  if (styled.length > 0) {
    consequences.push(
      `Key styles set for ${name} in ${styled.length === 1 ? `theme “${styled[0].name}”` : `${styled.length} themes`} will be removed.`,
    );
  }
  const appearance: AppearanceConfig = {
    ...config.appearance,
    defaultTheme: themeWithoutLayer(config.appearance.defaultTheme, idx),
    customThemes: (config.appearance.customThemes ?? []).map((t) => themeWithoutLayer(t, idx)),
  };

  return {
    ok: true,
    patch: {
      keymap: { ...km, keymaps },
      appearance,
      ...(profiles ? { profiles } : {}),
    },
    consequences,
    summary: `Deleted layer ${name}.`,
  };
}

/** Adds a fixed-actuation mode with the given name, or a copy of `copyOf`'s settings. */
export function addActuationMode(config: KeyglassConfig, name: string, copyOf?: number): ConfigEdit {
  const modes = config.actuationModes ?? [];
  const limit = maxActuationModes(config);
  if (modes.length >= limit) return refuse(`The keyboard holds at most ${limit} actuation modes.`);
  const trimmed = name.trim() || `Mode ${modes.length + 1}`;
  const source = copyOf !== undefined ? modes[copyOf] : undefined;
  const id = uniqueId(
    "mode",
    modes.map((m) => m.id),
  );
  const mode: ActuationMode = source
    ? {
        ...source,
        id,
        name: trimmed,
        firmwareDefault: undefined,
        touched: true,
        globalTouched: true,
        perKeyOverrides: Object.fromEntries(
          Object.entries(source.perKeyOverrides ?? {}).map(([layout, keys]) => [
            layout,
            Object.fromEntries(Object.entries(keys).map(([k, ov]) => [k, { ...ov, touched: true }])),
          ]),
        ),
      }
    : {
        id,
        name: trimmed,
        type: 1,
        actuationPointMm: 2.0,
        releasePointMm: 1.8,
        rapidTrigger: false,
        rapidTriggerSensitivityMm: 0.3,
        touched: true,
        globalTouched: true,
      };
  return {
    ok: true,
    patch: { actuationModes: [...modes.map((m) => ({ ...m, touched: m.touched === true })), mode] },
    consequences: [],
    summary: `Added actuation mode ${trimmed}.`,
  };
}

export function renameActuationMode(config: KeyglassConfig, idx: number, name: string): ConfigEdit {
  const trimmed = name.trim();
  const modes = config.actuationModes ?? [];
  if (!trimmed) return refuse("An actuation mode needs a name.");
  if (!modes[idx]) return refuse(`There is no actuation mode ${idx + 1}.`);
  return {
    ok: true,
    patch: {
      actuationModes: modes.map((m, i) =>
        i === idx ? { ...m, name: trimmed } : { ...m, touched: m.touched === true },
      ),
    },
    consequences: [],
    summary: `Renamed actuation mode to ${trimmed}.`,
  };
}

/**
 * Deletes the actuation mode at `idx`. The last mode can't be deleted. Profiles that use it
 * switch to the first remaining mode; profiles using later modes keep them.
 */
export function deleteActuationMode(config: KeyglassConfig, idx: number): ConfigEdit {
  const modes = config.actuationModes ?? [];
  const mode = modes[idx];
  if (!mode) return refuse(`There is no actuation mode ${idx + 1}.`);
  if (modes.length <= 1) return refuse("The keyboard needs at least one actuation mode.");
  const remaining = modes.filter((_, i) => i !== idx).map((m) => ({ ...m, touched: m.touched === true }));
  const fallbackName = remaining[0].name;
  const consequences: string[] = [];
  for (const p of config.profiles ?? []) {
    if ((p.actuation_mode_index ?? 0) === idx) {
      consequences.push(`Profile ${profileName(p)} uses “${mode.name}”; it will use “${fallbackName}”.`);
    }
  }
  const profiles = config.profiles?.map((p) => ({
    ...p,
    actuation_mode_index: shiftAfterDelete(p.actuation_mode_index ?? 0, idx, 0),
  }));
  return {
    ok: true,
    patch: { actuationModes: remaining, ...(profiles ? { profiles } : {}) },
    consequences,
    summary: `Deleted actuation mode ${mode.name}.`,
  };
}
