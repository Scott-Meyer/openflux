import type React from "react";
import { useId, useState } from "react";
import { parseKeySpec } from "./keyNames";
import type { DescribeContext, Part, PartState, RuleRow } from "./rules";
import {
  BUS_OPS,
  CORNER_NAMES,
  CORNERS,
  describeKeys,
  legendMode,
  opWords,
  PARTS,
  partState,
  setPartState,
  str,
} from "./rules";

export const BUILTIN_ICONS = [
  "cam",
  "play",
  "refresh",
  "brightness",
  "volume",
  "mic",
  "power",
  "star",
  "copy",
  "paste",
  "cut",
  "undo",
  "redo",
  "save",
  "search",
  "print",
  "close",
  "plus",
  "select_all",
];

export interface RuleEditorProps {
  row: RuleRow;
  /** `batch` edits (typing) may be held briefly before saving. */
  onChange: (next: RuleRow, batch?: boolean) => void;
  describe: DescribeContext;
  /** The keys picked on the map, formatted as a `keys` value, and how many there are. */
  selectionSpec: string;
  selectionCount: number;
  activeLayer: number;
  pickImage?: () => Promise<string | null>;
}

const LAYERS = Array.from({ length: 16 }, (_, i) => String(i));

type CondId = "layer" | "app" | "held" | "caps" | "time" | "idle" | "bus";

const CONDS: Array<{ id: CondId; label: string }> = [
  { id: "layer", label: "Layer" },
  { id: "app", label: "App" },
  { id: "held", label: "Key held" },
  { id: "caps", label: "Caps Lock" },
  { id: "time", label: "Time of day" },
  { id: "idle", label: "Idle" },
  { id: "bus", label: "Bus value" },
];

/** Whether the row already says something about this condition. */
function condInRow(row: RuleRow, id: CondId): boolean {
  switch (id) {
    case "layer":
      return str(row.layer) !== "" && str(row.layer) !== "any";
    case "app":
      return str(row.app) !== "";
    case "held":
      return str(row.held) === "held" || str(row.held) === "not_held";
    case "caps":
      return str(row.caps) === "on" || str(row.caps) === "off";
    case "time":
      return str(row.time) !== "";
    case "idle":
      return Number(row.idle_seconds) > 0;
    case "bus":
      return str(row.condition_topic) !== "";
  }
}

/** Fields a condition owns, reset when it's removed. */
const COND_RESET: Record<CondId, RuleRow> = {
  layer: { layer: "any" },
  app: { app: "" },
  held: { held: "any" },
  caps: { caps: "any" },
  time: { time: "" },
  idle: { idle_seconds: 0 },
  bus: { condition_topic: "", condition_path: "", condition_op: "eq", condition_value: "" },
};

const ColorField: React.FC<{
  label: string;
  value: string;
  placeholder?: string;
  onChange: (v: string, batch: boolean) => void;
}> = ({ label, value, placeholder, onChange }) => {
  const valid = /^#[0-9a-fA-F]{6}$/.test(value);
  return (
    <span className="kr-color">
      <input
        type="color"
        aria-label={`${label} swatch`}
        value={valid ? value : placeholder && /^#[0-9a-fA-F]{6}$/.test(placeholder) ? placeholder : "#ffffff"}
        onChange={(e) => onChange(e.target.value, true)}
      />
      <input
        type="text"
        aria-label={label}
        value={value}
        placeholder={placeholder ?? "#rrggbb"}
        onChange={(e) => onChange(e.target.value, true)}
      />
    </span>
  );
};

const StateSwitch: React.FC<{ label: string; state: PartState; onChange: (s: PartState) => void }> = ({
  label,
  state,
  onChange,
}) => (
  <fieldset className="kg-segmented kr-tri" aria-label={label}>
    {(["inherit", "set", "clear"] as PartState[]).map((s) => (
      <button
        key={s}
        type="button"
        aria-pressed={state === s}
        className={`kg-seg-btn${state === s ? " is-active" : ""}`}
        onClick={() => onChange(s)}
        title={
          s === "inherit"
            ? "Keep whatever is underneath (the keymap or another module)"
            : s === "set"
              ? "Set this part"
              : "Remove this part, even if something underneath sets it"
        }
      >
        {s === "inherit" ? "Inherit" : s === "set" ? "Set" : "Clear"}
      </button>
    ))}
  </fieldset>
);

/**
 * One Key Rule: which keys, when, and what they show. Everything edits the row in place, so
 * fields the editor doesn't know about survive.
 */
export const RuleEditor: React.FC<RuleEditorProps> = ({
  row,
  onChange,
  describe,
  selectionSpec,
  selectionCount,
  activeLayer,
  pickImage,
}) => {
  const uid = useId();
  const set = (patch: RuleRow, batch = false) => onChange({ ...row, ...patch }, batch);
  const parsed = parseKeySpec(row.keys, describe.resolver);
  const legend = legendMode(row);
  // Conditions added but not filled in yet (an empty app list says nothing in the row).
  const [opened, setOpened] = useState<ReadonlySet<CondId>>(new Set());
  const hasCond = (r: RuleRow, id: CondId) => condInRow(r, id) || opened.has(id);
  const missingConds = CONDS.filter((c) => !hasCond(row, c.id));
  const cornerStates = CORNERS.map((c) => partState(row, `corner_${c}`));
  const cornersInUse = cornerStates.some((st) => st !== "inherit");
  const [showCorners, setShowCorners] = useState(false);
  const cornersOpen = showCorners || cornersInUse;
  const cornersSummary = cornersInUse
    ? CORNERS.filter((_, i) => cornerStates[i] !== "inherit")
        .map((c) => `${CORNER_NAMES[c]} ${partState(row, `corner_${c}`) === "clear" ? "cleared" : "set"}`)
        .join(", ")
    : "inherit all four (text, a dot or an icon, inset from each corner)";

  const addCond = (id: CondId) => {
    setOpened((o) => new Set([...o, id]));
    switch (id) {
      case "layer":
        return set({ layer: String(activeLayer) });
      case "app":
        return;
      case "held":
        return set({ held: "held" });
      case "caps":
        return set({ caps: "on" });
      case "time":
        return;
      case "idle":
        return set({ idle_seconds: 60 });
      case "bus":
        return set({ condition_op: str(row.condition_op) || "eq" });
    }
  };
  const removeCond = (id: CondId) => {
    setOpened((o) => new Set([...o].filter((c) => c !== id)));
    set(COND_RESET[id]);
  };

  const partEditor = (part: Part): React.ReactNode => {
    switch (part) {
      case "label":
        return (
          <input
            type="text"
            aria-label="Label text"
            value={str(row.label)}
            onChange={(e) => set({ label: e.target.value }, true)}
          />
        );
      case "symbol":
        return (
          <span className="kr-inline">
            <select
              aria-label="Icon"
              value={str(row.icon)}
              onChange={(e) => set({ icon: e.target.value, image: e.target.value ? "" : row.image })}
            >
              <option value="">{str(row.image) ? "Image" : "Choose an icon"}</option>
              {BUILTIN_ICONS.map((ic) => (
                <option key={ic} value={ic}>
                  {ic}
                </option>
              ))}
            </select>
            {pickImage && (
              <button
                type="button"
                className="btn-secondary btn-compact"
                onClick={() =>
                  void pickImage().then((name) => {
                    if (name) set({ image: name, icon: "" });
                  })
                }
              >
                {str(row.image) ? str(row.image).split("/").pop() : "PNG…"}
              </button>
            )}
          </span>
        );
      case "color":
        return <ColorField label="Legend color" value={str(row.color)} onChange={(v, b) => set({ color: v }, b)} />;
      case "glow":
        return (
          <span className="kr-inline">
            <select
              aria-label="Glow style"
              value={str(row.glow) || "soft"}
              onChange={(e) => set({ glow: e.target.value })}
            >
              <option value="soft">Soft</option>
              <option value="subtle">Subtle</option>
              <option value="pulse">Pulsing</option>
            </select>
            <ColorField
              label="Glow color"
              value={str(row.glow_color)}
              placeholder="legend color"
              onChange={(v, b) => set({ glow_color: v }, b)}
            />
          </span>
        );
      case "outline":
        return (
          <span className="kr-inline">
            <select
              aria-label="Outline shape"
              value={str(row.outline) || "ring"}
              onChange={(e) => set({ outline: e.target.value })}
            >
              <option value="ring">Ring (centered)</option>
              <option value="window">Window (inset)</option>
              {str(row.outline) === "edge" && <option value="edge">Edge (shows parallax)</option>}
            </select>
            <ColorField
              label="Outline color"
              value={str(row.outline_color)}
              placeholder="legend color"
              onChange={(v, b) => set({ outline_color: v }, b)}
            />
            <label className="checkbox-inline">
              <input
                type="checkbox"
                checked={row.outline_pulse === true}
                onChange={(e) => set({ outline_pulse: e.target.checked })}
              />
              Pulse
            </label>
          </span>
        );
      case "backdrop":
        return (
          <ColorField label="Backdrop color" value={str(row.backdrop)} onChange={(v, b) => set({ backdrop: v }, b)} />
        );
      default: {
        const key = part;
        const v = str(row[key]);
        return (
          <input
            type="text"
            aria-label={`${CORNER_NAMES[key.slice(7) as (typeof CORNERS)[number]]} corner`}
            value={v}
            placeholder="text, dot, or icon:star"
            onChange={(e) => set({ [key]: e.target.value }, true)}
          />
        );
      }
    }
  };

  return (
    <div className="kr-editor">
      <div className="kr-section">
        <div className="kr-section-title">Keys</div>
        <div className="kr-inline">
          <input
            id={`${uid}-keys`}
            type="text"
            aria-label="Keys"
            value={str(row.keys)}
            placeholder="F1-F4, Esc, home_row, action:<id>"
            onChange={(e) => set({ keys: e.target.value }, true)}
          />
          <button
            type="button"
            className="btn-secondary btn-compact"
            disabled={selectionCount === 0 || selectionSpec === str(row.keys)}
            onClick={() => set({ keys: selectionSpec })}
          >
            Use selected ({selectionCount})
          </button>
        </div>
        <small className="muted">
          {parsed.indices.length === 0
            ? "Targets no keys yet."
            : `${parsed.indices.length} key${parsed.indices.length === 1 ? "" : "s"}: ${describeKeys(row.keys, describe)}.`}
          {parsed.unknown.length > 0 && <span className="error-text"> Unknown: {parsed.unknown.join(", ")}.</span>}
        </small>
      </div>

      <div className="kr-section">
        <div className="kr-section-title">When</div>
        {CONDS.filter((c) => hasCond(row, c.id)).length === 0 && (
          <small className="muted">Always (on every layer, in every app).</small>
        )}
        {hasCond(row, "layer") && (
          <div className="kr-cond">
            <span>On layer</span>
            <select aria-label="Layer" value={str(row.layer)} onChange={(e) => set({ layer: e.target.value })}>
              {LAYERS.map((l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="kr-x"
              aria-label="Remove layer condition"
              onClick={() => removeCond("layer")}
            >
              ✕
            </button>
          </div>
        )}
        {hasCond(row, "app") && (
          <div className="kr-cond">
            <span>In apps</span>
            <input
              type="text"
              aria-label="Apps"
              value={str(row.app)}
              placeholder="obs, zoom"
              onChange={(e) => set({ app: e.target.value }, true)}
            />
            <button type="button" className="kr-x" aria-label="Remove app condition" onClick={() => removeCond("app")}>
              ✕
            </button>
          </div>
        )}
        {hasCond(row, "held") && (
          <div className="kr-cond">
            <span>While the key is</span>
            <select aria-label="Held" value={str(row.held)} onChange={(e) => set({ held: e.target.value })}>
              <option value="held">held</option>
              <option value="not_held">not held</option>
            </select>
            <button
              type="button"
              className="kr-x"
              aria-label="Remove held condition"
              onClick={() => removeCond("held")}
            >
              ✕
            </button>
          </div>
        )}
        {hasCond(row, "caps") && (
          <div className="kr-cond">
            <span>While Caps Lock is</span>
            <select aria-label="Caps Lock" value={str(row.caps)} onChange={(e) => set({ caps: e.target.value })}>
              <option value="on">on</option>
              <option value="off">off</option>
            </select>
            <button
              type="button"
              className="kr-x"
              aria-label="Remove Caps Lock condition"
              onClick={() => removeCond("caps")}
            >
              ✕
            </button>
          </div>
        )}
        {hasCond(row, "time") && (
          <div className="kr-cond">
            <span>Between</span>
            <input
              type="text"
              aria-label="Time range"
              value={str(row.time)}
              placeholder="22:00-06:00"
              onChange={(e) => set({ time: e.target.value }, true)}
            />
            <button
              type="button"
              className="kr-x"
              aria-label="Remove time condition"
              onClick={() => removeCond("time")}
            >
              ✕
            </button>
          </div>
        )}
        {hasCond(row, "idle") && (
          <div className="kr-cond">
            <span>After idle for</span>
            <input
              type="number"
              aria-label="Idle seconds"
              min={1}
              value={Number(row.idle_seconds) || 0}
              onChange={(e) => set({ idle_seconds: Math.max(0, Number(e.target.value) || 0) }, true)}
            />
            <span>seconds</span>
            <button
              type="button"
              className="kr-x"
              aria-label="Remove idle condition"
              onClick={() => removeCond("idle")}
            >
              ✕
            </button>
          </div>
        )}
        {hasCond(row, "bus") && (
          <div className="kr-cond is-bus">
            <span>When the bus value</span>
            <input
              type="text"
              aria-label="Bus topic"
              value={str(row.condition_topic)}
              placeholder="modules/mqtt-bridge/status"
              onChange={(e) => set({ condition_topic: e.target.value }, true)}
            />
            <button type="button" className="kr-x" aria-label="Remove bus condition" onClick={() => removeCond("bus")}>
              ✕
            </button>
            <span className="kr-cond-break" />
            <span>at</span>
            <input
              type="text"
              aria-label="Value path"
              value={str(row.condition_path)}
              placeholder="path (optional), e.g. state"
              onChange={(e) => set({ condition_path: e.target.value }, true)}
            />
            <select
              aria-label="Comparison"
              value={str(row.condition_op) || "eq"}
              onChange={(e) => set({ condition_op: e.target.value })}
            >
              {BUS_OPS.map((op) => (
                <option key={op} value={op}>
                  {opWords(op)}
                </option>
              ))}
            </select>
            {str(row.condition_op) !== "exists" && str(row.condition_op) !== "missing" && (
              <input
                type="text"
                aria-label="Value"
                value={str(row.condition_value)}
                placeholder={str(row.condition_op) === "in" ? "a, b, c" : "value"}
                onChange={(e) => set({ condition_value: e.target.value }, true)}
              />
            )}
          </div>
        )}
        {missingConds.length > 0 && (
          <div className="kr-add-row">
            <span className="muted">Only…</span>
            {missingConds.map((c) => (
              <button key={c.id} type="button" className="kr-chip" onClick={() => addCond(c.id)}>
                + {c.label}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="kr-section">
        <div className="kr-section-title">Look</div>
        <div className="kr-part">
          <span className="kr-part-name">Legend</span>
          <fieldset className="kg-segmented" aria-label="Legend">
            {(
              [
                ["keep", "Keep"],
                ["legend", "Hide legend"],
                ["all", "Blank key"],
              ] as const
            ).map(([v, l]) => (
              <button
                key={v}
                type="button"
                aria-pressed={legend === v}
                className={`kg-seg-btn${legend === v ? " is-active" : ""}`}
                onClick={() => set({ hide: v === "keep" ? "nothing" : v })}
              >
                {l}
              </button>
            ))}
          </fieldset>
        </div>
        <small className="muted kr-part-hint">
          {legend === "legend"
            ? "Hides the label and icon. Layer-tap and modifier markers, glows and corner text stay."
            : legend === "all"
              ? "An empty key: nothing underneath shows, not even markers or signals."
              : "The legend from the keymap (or another module) stays."}
        </small>
        {legend !== "all" &&
          PARTS.filter((p) => !p.id.startsWith("corner_")).map((p) => {
            const st = partState(row, p.id);
            return (
              <div key={p.id} className={`kr-part${st === "set" ? " is-set" : ""}`}>
                <span className="kr-part-name" title={p.hint}>
                  {p.label}
                </span>
                <StateSwitch label={p.label} state={st} onChange={(s) => onChange(setPartState(row, p.id, s))} />
                {st === "set" && <span className="kr-part-value">{partEditor(p.id)}</span>}
                {st === "clear" && (
                  <span className="muted kr-part-value">removed, even if something underneath sets it</span>
                )}
              </div>
            );
          })}
        {legend !== "all" && (
          <div className="kr-part">
            <span className="kr-part-name">Corners</span>
            <button
              type="button"
              className="link-btn"
              aria-expanded={cornersOpen}
              onClick={() => setShowCorners((v) => !v)}
              disabled={cornersInUse}
            >
              {cornersOpen ? "▾" : "▸"} {cornersSummary}
            </button>
          </div>
        )}
        {legend !== "all" &&
          cornersOpen &&
          PARTS.filter((p) => p.id.startsWith("corner_")).map((p) => {
            const st = partState(row, p.id);
            return (
              <div key={p.id} className={`kr-part is-corner${st === "set" ? " is-set" : ""}`}>
                <span className="kr-part-name" title={p.hint}>
                  {p.label.replace(" corner", "")}
                </span>
                <StateSwitch label={p.label} state={st} onChange={(s) => onChange(setPartState(row, p.id, s))} />
                {st === "set" && <span className="kr-part-value">{partEditor(p.id)}</span>}
                {st === "clear" && (
                  <span className="muted kr-part-value">
                    {p.id === "corner_tr" ? "removes layer-tap and modifier markers too" : "removed"}
                  </span>
                )}
              </div>
            );
          })}
        <small className="muted">
          Everything is drawn around the key's center or inset from its edge, so it stays lined up through the keycap at
          any viewing angle.
        </small>
      </div>
    </div>
  );
};
