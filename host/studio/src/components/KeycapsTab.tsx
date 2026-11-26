// The Keycaps tab: where people build Key Rules (the `keys-custom` module), starting from the
// keyboard. Pick keys on the map (or a group), say what they should show and when, and see it on
// the map. The side panel asks the renderer why the focused key looks the way it does.
//
// Rules are the module's `rules` setting, the same rows `fluxctl keys rules …` and
// `fluxctl modules config keys-custom --set rules='[...]'` edit. Studio patches rows in place, so
// fields it doesn't know about survive, and a change made elsewhere shows up here.
import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { v2Client } from "../api/v2Client";
import type { Face, ProviderPayload } from "./keyRules/faces";
import { resolveFaces } from "./keyRules/faces";
import type { MapKey } from "./keyRules/KeyFaceMap";
import { KeyFaceMap } from "./keyRules/KeyFaceMap";
import { buildKeyResolver, formatKeySpec, KEY_GROUPS, parseKeySpec } from "./keyRules/keyNames";
import { OverrideEditor } from "./keyRules/OverrideEditor";
import { RuleEditor } from "./keyRules/RuleEditor";
import { RuleList } from "./keyRules/RuleList";
import type { CustomOverride, DescribeContext, RuleRow } from "./keyRules/rules";
import { customProviderRules, describeKeys, newRule, QUICK_LOOKS } from "./keyRules/rules";
import { WhyPanel } from "./keyRules/WhyPanel";

interface ActionEntry {
  slot: number;
  keycode: number;
  action_id: string;
  module_id?: string;
  label?: string;
  accent?: string;
  mode?: string;
}

interface KeycapsTabProps {
  pickFile?: (title: string) => Promise<string | null>;
  /** Key to focus first (e.g. opened from Home's "Override"). */
  initialKey?: number | null;
}

const CUSTOM_TOPIC = "keys/providers/custom";

const ICON_NAMES: Record<string, string> = {
  kbd_up: "Up",
  kbd_down: "Down",
  kbd_left: "Left",
  kbd_right: "Right",
  kbd_space: "Space",
  kbd_shift: "Shift",
  kbd_enter: "Enter",
  kbd_bksp: "Backspace",
  kbd_tab: "Tab",
  kbd_caps: "Caps Lock",
  kbd_ctrl: "Ctrl",
  kbd_cmd: "Cmd",
  kbd_alt: "Alt",
  kbd_layer: "Fn",
  kbd_home: "Home",
  kbd_end: "End",
  kbd_pgup: "PgUp",
  kbd_pgdn: "PgDn",
};

function formatHexKeycode(kc: number): string {
  return `0x${kc.toString(16).toUpperCase().padStart(4, "0")}`;
}

function storedRules(v: unknown): unknown[] {
  if (Array.isArray(v)) return v;
  if (typeof v === "string") {
    try {
      const p = JSON.parse(v);
      return Array.isArray(p) ? p : [];
    } catch {
      return [];
    }
  }
  return [];
}

export const KeycapsTab: React.FC<KeycapsTabProps> = ({ pickFile, initialKey }) => {
  const [keys, setKeys] = useState<MapKey[]>([]);
  const [resolvedKeycodes, setResolvedKeycodes] = useState<number[]>([]);
  const [activeLayer, setActiveLayer] = useState(0);
  const [actionsByKc, setActionsByKc] = useState<Record<number, ActionEntry>>({});
  const [providers, setProviders] = useState<ProviderPayload[]>([]);
  const [keymap, setKeymap] = useState<unknown>(undefined);
  const [byKey, setByKey] = useState<Record<string, CustomOverride>>({});
  const [byAction, setByAction] = useState<Record<string, CustomOverride>>({});
  const [moduleEnabled, setModuleEnabled] = useState<boolean | null>(null);
  const [rows, setRows] = useState<unknown[]>([]);
  const [loaded, setLoaded] = useState(false);

  const [focusKey, setFocusKey] = useState<number>(initialKey ?? 0);
  const [selection, setSelection] = useState<number[]>([initialKey ?? 0]);
  const [selectionGroup, setSelectionGroup] = useState<string | null>(null);
  const [openIndex, setOpenIndex] = useState<number | null>(null);
  // The map shows one layer, so a new rule starts on that layer; unticking makes it every layer.
  const [onlyThisLayer, setOnlyThisLayer] = useState(true);
  const [status, setStatus] = useState<string | null>(null);
  const [savingOverride, setSavingOverride] = useState(false);
  const [savedTick, setSavedTick] = useState(0);

  // ----- Saving rows: batched while typing, never overwritten by a poll while unsent -----
  const serverRows = useRef<string>("");
  // Bumped on every local edit: a poll that started before an edit must not apply its (older) rows.
  const editGen = useRef(0);
  const pending = useRef<{ rows: unknown[] | null; timer: number | null; inflight: boolean }>({
    rows: null,
    timer: null,
    inflight: false,
  });
  const syncTimer = useRef<number | null>(null);
  const enabledRef = useRef<boolean | null>(null);
  enabledRef.current = moduleEnabled;

  const scheduleSync = useCallback(() => {
    if (syncTimer.current !== null) window.clearTimeout(syncTimer.current);
    syncTimer.current = window.setTimeout(() => {
      syncTimer.current = null;
      void v2Client.syncToKeyboard().catch(() => {});
    }, 600);
  }, []);

  const flush = useCallback(async () => {
    const p = pending.current;
    if (p.timer !== null) {
      window.clearTimeout(p.timer);
      p.timer = null;
    }
    if (p.inflight || !p.rows) return;
    const toSave = p.rows;
    p.rows = null;
    p.inflight = true;
    try {
      await v2Client.updateModuleState("keys-custom", {
        ...(enabledRef.current === false ? { enabled: true } : {}),
        config: { rules: toSave },
      });
      serverRows.current = JSON.stringify(toSave);
      if (enabledRef.current === false) setModuleEnabled(true);
      setStatus(null);
      setSavedTick((t) => t + 1);
      scheduleSync();
    } catch (err) {
      setStatus(`Couldn't save Custom Legends: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      p.inflight = false;
      if (p.rows) void flush();
    }
  }, [scheduleSync]);

  const commitRows = useCallback(
    (next: unknown[], batch = false) => {
      editGen.current += 1;
      setRows(next);
      const p = pending.current;
      p.rows = next;
      if (p.timer !== null) window.clearTimeout(p.timer);
      p.timer = window.setTimeout(() => void flush(), batch ? 450 : 0);
    },
    [flush],
  );

  useEffect(
    () => () => {
      void flush();
      if (syncTimer.current !== null) window.clearTimeout(syncTimer.current);
    },
    [flush],
  );

  // ----- Live state -----
  const load = useCallback(async () => {
    const genAtStart = editGen.current;
    try {
      const [geomRes, rkRes, actRes, provRes, kmRes, modsRes] = await Promise.all([
        v2Client.getBusRetained("keyboard/geometry"),
        v2Client.getBusRetained("keyboard/resolved_keys"),
        v2Client.getBusRetained("system/actions"),
        v2Client.getBusRetained("keys/providers/#"),
        v2Client.getBusRetained("keyboard/keymap").catch(() => ({ messages: [] })),
        v2Client.listModules(),
      ]);
      const g = geomRes.messages[0]?.payload as { keys?: Array<MapKey & { key_index?: number }> } | undefined;
      if (Array.isArray(g?.keys)) {
        setKeys(
          g.keys.map((k, i) => ({
            index: typeof k.key_index === "number" ? k.key_index : typeof k.index === "number" ? k.index : i,
            rect: k.rect,
            row: k.row,
          })),
        );
      }
      const rk = rkRes.messages[0]?.payload as { active_layer?: number; keys?: Array<{ keycode: number }> } | undefined;
      if (typeof rk?.active_layer === "number") setActiveLayer(rk.active_layer);
      if (Array.isArray(rk?.keys)) setResolvedKeycodes(rk.keys.map((k) => Number(k?.keycode)));
      const act = actRes.messages[0]?.payload as
        | { slots?: Record<string, Omit<ActionEntry, "slot" | "keycode"> & { keycode?: number }> }
        | undefined;
      const actMap: Record<number, ActionEntry> = {};
      for (const [slot, e] of Object.entries(act?.slots ?? {})) {
        const kc = typeof e.keycode === "number" ? e.keycode : 0x7e00 + Number(slot);
        actMap[kc] = { ...e, slot: Number(slot), keycode: kc };
      }
      setActionsByKc(actMap);
      setProviders(provRes.messages.map((m) => ({ topic: m.topic, payload: m.payload })));
      setKeymap(kmRes.messages[0]?.payload);

      const mod = modsRes.modules.find((m) => m.id === "keys-custom");
      setModuleEnabled(mod ? mod.state.enabled : null);
      const cfg = (mod?.state.config ?? {}) as {
        rules?: unknown;
        by_key?: Record<string, CustomOverride>;
        by_action?: Record<string, CustomOverride>;
      };
      setByKey(cfg.by_key ?? {});
      setByAction(cfg.by_action ?? {});
      const stored = storedRules(cfg.rules);
      const json = JSON.stringify(stored);
      const p = pending.current;
      const editedSince = genAtStart !== editGen.current;
      if (json !== serverRows.current && !editedSince && !p.rows && !p.inflight && p.timer === null) {
        serverRows.current = json;
        setRows(stored);
      }
      setLoaded(true);
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 2500);
    return () => window.clearInterval(t);
  }, [load]);

  // ----- Derived: names, faces -----
  const actionKeycodes = useMemo(() => {
    const m: Record<string, number> = {};
    for (const a of Object.values(actionsByKc)) m[a.action_id] = a.keycode;
    return m;
  }, [actionsByKc]);
  const resolver = useMemo(
    () => buildKeyResolver({ keys, keymap, resolvedKeycodes, actionKeycodes }),
    [keys, keymap, resolvedKeycodes, actionKeycodes],
  );

  const baseFaces = useMemo(
    () =>
      resolveFaces(
        providers.filter((p) => p.topic !== CUSTOM_TOPIC),
        { resolver, activeLayer },
      ),
    [providers, resolver, activeLayer],
  );
  const keyName = useCallback(
    (i: number): string => {
      const f: Face | undefined = baseFaces.get(i);
      const label = f?.label?.trim();
      if (label) return label.length === 1 ? label.toUpperCase() : label;
      if (f?.icon && ICON_NAMES[f.icon]) return ICON_NAMES[f.icon];
      const tok = resolver.tokenFor(i);
      return tok.startsWith("#") ? `Key ${i}` : tok === "comma" ? "," : tok.replace(/^KC_/, "");
    },
    [baseFaces, resolver],
  );
  const describe: DescribeContext = useMemo(() => ({ resolver, keyName }), [resolver, keyName]);

  const customRules = useMemo(
    () => customProviderRules({ rules: rows, by_key: byKey, by_action: byAction }, resolver),
    [rows, byKey, byAction, resolver],
  );
  const faces = useMemo(
    () =>
      resolveFaces(providers, {
        resolver,
        activeLayer,
        overrideTopic: { topic: CUSTOM_TOPIC, priority: 100, mode: "patch", rules: customRules },
        previewSource: openIndex !== null ? `rules:${openIndex + 1}` : null,
      }),
    [providers, resolver, activeLayer, customRules, openIndex],
  );

  const openRow = openIndex !== null ? (rows[openIndex] as RuleRow | undefined) : undefined;
  const highlight = useMemo(
    () => new Set(openRow ? parseKeySpec(openRow.keys, resolver).indices : []),
    [openRow, resolver],
  );
  const selectionSet = useMemo(() => new Set(selection), [selection]);
  const selectionSpec = useMemo(
    () => formatKeySpec(selection, resolver, selectionGroup),
    [selection, resolver, selectionGroup],
  );

  const overridden = useMemo(() => {
    const s = new Set<number>();
    for (const k of Object.keys(byKey)) {
      const m = /^([^:]+):(\d+)$/.exec(k);
      if (m && (m[1] === "*" || Number(m[1]) === activeLayer)) s.add(Number(m[2]));
    }
    resolvedKeycodes.forEach((kc, i) => {
      const a = actionsByKc[kc];
      if (a && byAction[a.action_id]) s.add(i);
    });
    return s;
  }, [byKey, byAction, activeLayer, resolvedKeycodes, actionsByKc]);
  const actionAccent = useMemo(() => {
    const m = new Map<number, string>();
    resolvedKeycodes.forEach((kc, i) => {
      const a = actionsByKc[kc];
      if (a) m.set(i, a.accent || "#fa6831");
    });
    return m;
  }, [resolvedKeycodes, actionsByKc]);

  // ----- Interactions -----
  const onKeyClick = (i: number, additive: boolean) => {
    setFocusKey(i);
    setSelectionGroup(null);
    if (additive) setSelection((s) => (s.includes(i) ? s.filter((x) => x !== i) : [...s, i]));
    else setSelection([i]);
  };
  const pickGroup = (g: string) => {
    setSelection(resolver.group(g));
    setSelectionGroup(g);
  };
  const addRule = (look: RuleRow) => {
    const row = newRule(selectionSpec, { ...look, layer: onlyThisLayer ? String(activeLayer) : "any" });
    commitRows([...rows, row]);
    setOpenIndex(rows.length);
  };
  const changeRow = (i: number, next: RuleRow, batch?: boolean) => {
    const copy = [...rows];
    copy[i] = next;
    commitRows(copy, batch);
  };
  const openRule = (i: number | null) => {
    setOpenIndex(i);
    if (i === null) return;
    const r = rows[i] as RuleRow | undefined;
    const ks = r ? parseKeySpec(r.keys, resolver).indices : [];
    if (ks.length > 0 && !ks.includes(focusKey)) setFocusKey(ks[0]);
  };

  const pickImage = pickFile
    ? async (): Promise<string | null> => {
        const local = await pickFile("Choose a PNG for these keys");
        if (!local) return null;
        try {
          return (await v2Client.importMediaFile(local)).name;
        } catch (err) {
          setStatus(err instanceof Error ? err.message : String(err));
          return null;
        }
      }
    : undefined;

  const saveOverrides = async (
    nextByKey: Record<string, CustomOverride>,
    nextByAction: Record<string, CustomOverride>,
    message: string,
  ) => {
    setSavingOverride(true);
    try {
      await v2Client.updateModuleState("keys-custom", {
        ...(moduleEnabled === false ? { enabled: true } : {}),
        config: { by_key: nextByKey, by_action: nextByAction },
      });
      setByKey(nextByKey);
      setByAction(nextByAction);
      setStatus(message);
      setSavedTick((t) => t + 1);
      scheduleSync();
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingOverride(false);
    }
  };

  const focusAction = actionsByKc[resolvedKeycodes[focusKey]];
  const focusName = keyName(focusKey);
  const explainCtx = useMemo(
    () => ({
      providers: Object.fromEntries(providers.map((p) => [p.topic, p.payload])),
      rows: rows.filter((r): r is RuleRow => typeof r === "object" && r !== null),
      activeLayer,
    }),
    [providers, rows, activeLayer],
  );

  return (
    <div className="kr-page">
      <section className="card kr-board" aria-label="Keyboard">
        <div className="kr-board-head">
          <div>
            <h3>Pick keys</h3>
            <small className="muted">
              Click a key to pick it, Shift- or Cmd-click to add more, or pick a group. Layer {activeLayer} is active.
            </small>
          </div>
          <button type="button" className="btn-secondary btn-compact" onClick={() => void load()}>
            Refresh
          </button>
        </div>
        <fieldset className="kr-groups" aria-label="Key groups">
          {KEY_GROUPS.map((g) => (
            <button
              key={g.id}
              type="button"
              className={`kr-chip${selectionGroup === g.id ? " is-active" : ""}`}
              onClick={() => pickGroup(g.id)}
            >
              {g.label}
            </button>
          ))}
          {selection.length > 0 && (
            <button
              type="button"
              className="kr-chip is-quiet"
              onClick={() => {
                setSelection([]);
                setSelectionGroup(null);
              }}
            >
              Clear selection
            </button>
          )}
        </fieldset>
        <KeyFaceMap
          keys={keys}
          faces={faces}
          selection={selectionSet}
          focusKey={focusKey}
          highlight={highlight}
          overridden={overridden}
          actionAccent={actionAccent}
          keyName={keyName}
          onKeyClick={onKeyClick}
        />
        <div className="kr-map-legend muted">
          <span>
            <span className="kr-swatch is-selected" /> picked
          </span>
          {openRow && (
            <span>
              <span className="kr-swatch is-highlight" /> keys of rule {(openIndex ?? 0) + 1} (previewed as if its
              conditions hold)
            </span>
          )}
          <span>
            <span className="kr-swatch is-override" /> quick override
          </span>
          <span>
            <span className="kr-swatch is-action" /> module action
          </span>
          <span>The map is a rough preview; the side panel asks the keyboard for the exact answer.</span>
        </div>
        <fieldset className="kr-composer" aria-label="New rule">
          <span className="kr-composer-tag">New rule</span>
          <span className="kr-composer-lead">
            {selection.length === 0 ? (
              <span className="muted">Pick keys to start a rule.</span>
            ) : (
              <>
                On <strong>{describeKeys(selectionSpec, describe)}</strong>
              </>
            )}
          </span>
          <label className="checkbox-inline">
            <input type="checkbox" checked={onlyThisLayer} onChange={(e) => setOnlyThisLayer(e.target.checked)} />
            only on layer {activeLayer}
          </label>
          <span className="kr-composer-looks">
            {QUICK_LOOKS.map((q) => (
              <button
                key={q.id}
                type="button"
                className="btn-secondary btn-compact"
                disabled={selection.length === 0}
                onClick={() => addRule(q.look)}
              >
                {q.label}
              </button>
            ))}
          </span>
        </fieldset>
      </section>

      <div className="kr-columns">
        <section className="card kr-rules-card" aria-label="Custom Legends rules">
          <div className="kr-board-head">
            <div>
              <h3>Your rules</h3>
              <small className="muted">
                Later rules win over earlier ones. All rules win over modules and the keymap.
              </small>
            </div>
          </div>
          {moduleEnabled === false && (
            <div className="notice notice-warn">Custom Legends is turned off. Saving a rule turns it back on.</div>
          )}
          {loaded && (
            <RuleList
              rows={rows}
              openIndex={openIndex}
              describe={describe}
              onOpen={openRule}
              onChangeRows={commitRows}
              renderEditor={(row, i) => (
                <RuleEditor
                  key={i}
                  row={row}
                  onChange={(next, batch) => changeRow(i, next, batch)}
                  describe={describe}
                  selectionSpec={selectionSpec}
                  selectionCount={selection.length}
                  activeLayer={activeLayer}
                  pickImage={pickImage}
                />
              )}
            />
          )}
          {status && (
            <div className="status-msg" role="status">
              {status}
            </div>
          )}
          <small className="muted kr-cli-note">
            The same list as <code>fluxctl keys rules list</code> and <code>fluxctl modules config keys-custom</code>.
          </small>
        </section>

        <div className="kr-side">
          <div className="kr-binding muted">
            <strong>{focusName}</strong> · key {focusKey} ·{" "}
            {focusAction ? (
              <span style={{ color: focusAction.accent || "#fa6831" }}>
                {focusAction.label || focusAction.action_id} ({focusAction.module_id ?? "built-in"} ·{" "}
                {focusAction.mode ?? "tap"})
              </span>
            ) : resolvedKeycodes[focusKey] !== undefined ? (
              <>bound to {formatHexKeycode(resolvedKeycodes[focusKey])}</>
            ) : (
              "binding unknown"
            )}
          </div>
          <WhyPanel
            keyIndex={focusKey}
            keyName={focusName}
            context={explainCtx}
            refreshToken={savedTick}
            pulseKeys={selection.includes(focusKey) ? selection : [focusKey]}
            onOpenRule={(i) => openRule(i)}
          />
          <OverrideEditor
            keyIndex={focusKey}
            keyName={focusName}
            activeLayer={activeLayer}
            action={focusAction}
            byKey={byKey}
            byAction={byAction}
            onSave={saveOverrides}
            saving={savingOverride}
            pickFile={pickFile}
          />
        </div>
      </div>
    </div>
  );
};
