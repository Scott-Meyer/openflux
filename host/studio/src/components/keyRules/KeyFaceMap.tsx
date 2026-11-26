import type React from "react";
import { useMemo } from "react";
import type { Face } from "./faces";

export interface MapKey {
  index: number;
  rect: { x: number; y: number; w: number; h: number };
  row?: number;
}

export interface KeyFaceMapProps {
  keys: MapKey[];
  faces: Map<number, Face>;
  /** Keys picked for a new rule or for "use selection". */
  selection: ReadonlySet<number>;
  /** The key the side panel explains. */
  focusKey: number;
  /** Keys the open rule targets (drawn with a dashed ring). */
  highlight?: ReadonlySet<number>;
  /** Small cyan dot: this key has an older per-key/per-action override. */
  overridden?: ReadonlySet<number>;
  /** Small orange dot: a module action is bound here. */
  actionAccent?: ReadonlyMap<number, string>;
  /** Accessible name per key ("F2", "A"). */
  keyName: (index: number) => string;
  onKeyClick: (index: number, additive: boolean) => void;
}

const DEFAULT_FX = "#ffd166";

/** A short stand-in for an icon name on the map (the real icons are drawn by the renderer). */
function iconGlyph(icon: string): string {
  const map: Record<string, string> = {
    play: "▶",
    volume: "🔊",
    mic: "🎙",
    power: "⏻",
    star: "★",
    search: "⌕",
    close: "✕",
    plus: "+",
    brightness: "☀",
    refresh: "↻",
    undo: "↶",
    redo: "↷",
    save: "💾",
    copy: "⧉",
    cam: "◉",
    kbd_up: "↑",
    kbd_down: "↓",
    kbd_left: "←",
    kbd_right: "→",
    kbd_shift: "⇧",
    kbd_enter: "⏎",
    kbd_bksp: "⌫",
    kbd_tab: "⇥",
    kbd_caps: "⇪",
    kbd_space: "␣",
    kbd_ctrl: "⌃",
    kbd_cmd: "⌘",
    kbd_alt: "⌥",
    kbd_layer: "Fn",
  };
  return map[icon] ?? "◆";
}

function cornerContent(item: {
  text?: string;
  icon?: string;
  dot?: boolean | string;
  color?: string;
}): React.ReactNode {
  if (item.dot !== undefined) {
    const on = item.dot === true || item.dot === "on";
    return (
      <span
        className="kr-corner-dot"
        style={{ background: on ? (item.color ?? "#59eb8c") : "rgba(148,163,184,0.5)" }}
      />
    );
  }
  if (item.icon) return iconGlyph(item.icon);
  return item.text ?? "";
}

/**
 * The keyboard as a grid of little screens, drawn roughly the way the renderer will: legend in
 * the middle, corner items inset, glow / outline / backdrop inset from the keycap edge.
 * Click picks a key; Shift/Cmd/Ctrl-click adds or removes it from the selection.
 */
export const KeyFaceMap: React.FC<KeyFaceMapProps> = ({
  keys,
  faces,
  selection,
  focusKey,
  highlight,
  overridden,
  actionAccent,
  keyName,
  onKeyClick,
}) => {
  const bounds = useMemo(() => {
    if (keys.length === 0) return { minX: 0, minY: 0, w: 1920, h: 820 };
    let minX = Infinity;
    let minY = Infinity;
    let maxX = 0;
    let maxY = 0;
    for (const k of keys) {
      minX = Math.min(minX, k.rect.x);
      minY = Math.min(minY, k.rect.y);
      maxX = Math.max(maxX, k.rect.x + k.rect.w);
      maxY = Math.max(maxY, k.rect.y + k.rect.h);
    }
    return { minX, minY, w: Math.max(1, maxX - minX), h: Math.max(1, maxY - minY) };
  }, [keys]);

  return (
    <fieldset className="kr-map" style={{ aspectRatio: `${bounds.w} / ${bounds.h}` }} aria-label="Keyboard">
      {keys.length === 0 && <div className="kr-map-empty muted">Waiting for the keyboard layout…</div>}
      {keys.map((k) => {
        const face = faces.get(k.index) ?? {};
        const blank = face.blank === true;
        const label = blank ? "" : (face.label ?? "");
        const symbol = blank ? "" : face.image ? "▣" : face.icon ? iconGlyph(face.icon) : "";
        const legendColor = face.color ?? "#eef4ff";
        const glow = blank ? undefined : face.glow;
        const outline = blank ? undefined : face.outline;
        const backdrop = blank ? undefined : face.backdrop;
        const isSel = selection.has(k.index);
        const isFocus = focusKey === k.index;
        const name = keyName(k.index);
        const style: React.CSSProperties = {
          left: `${((k.rect.x - bounds.minX) / bounds.w) * 100}%`,
          top: `${((k.rect.y - bounds.minY) / bounds.h) * 100}%`,
          width: `${(k.rect.w / bounds.w) * 100}%`,
          height: `${(k.rect.h / bounds.h) * 100}%`,
        };
        const glowColor = glow?.color ?? face.color ?? DEFAULT_FX;
        const classes = ["kr-key"];
        if (isSel) classes.push("is-selected");
        if (isFocus) classes.push("is-focus");
        if (highlight?.has(k.index)) classes.push("is-highlight");
        if (blank) classes.push("is-blank");
        return (
          <button
            key={k.index}
            type="button"
            className={classes.join(" ")}
            style={style}
            aria-label={name}
            aria-pressed={isSel}
            data-key-index={k.index}
            title={`${name} · key ${k.index}${isSel ? " · selected" : ""}`}
            onClick={(e) => onKeyClick(k.index, e.shiftKey || e.metaKey || e.ctrlKey)}
          >
            <span className="kr-key-screen">
              {backdrop && (
                <span
                  className={`kr-fx-backdrop${backdrop.shape === "edge" ? " is-edge" : ""}`}
                  style={{
                    background: backdrop.color ?? DEFAULT_FX,
                    opacity: Math.min(1, (backdrop.strength ?? 0.55) + 0.2),
                  }}
                />
              )}
              {glow && (
                <span
                  className={`kr-fx-glow${glow.pulse ? " is-pulse" : ""}`}
                  style={{
                    background: `radial-gradient(circle, ${glowColor} 0%, transparent 68%)`,
                    opacity: 0.35 + 0.5 * (glow.strength ?? 1),
                  }}
                />
              )}
              {outline && (
                <span
                  className={`kr-fx-outline is-${outline.shape ?? "ring"}${outline.pulse ? " is-pulse" : ""}`}
                  style={{ borderColor: outline.color ?? face.color ?? DEFAULT_FX }}
                />
              )}
              {(label || symbol) && (
                <span className="kr-legend" style={{ color: legendColor }}>
                  {symbol && <span className="kr-legend-symbol">{symbol}</span>}
                  {label && <span className="kr-legend-label">{label}</span>}
                </span>
              )}
              {!blank &&
                face.corners &&
                (Object.entries(face.corners) as Array<[string, NonNullable<Face["corners"]>["tl"]]>).map(
                  ([slot, item]) =>
                    item ? (
                      <span key={slot} className={`kr-corner is-${slot}`} style={{ color: item.color ?? legendColor }}>
                        {cornerContent(item)}
                      </span>
                    ) : null,
                )}
            </span>
            {actionAccent?.has(k.index) && (
              <span className="kr-mark is-action" style={{ background: actionAccent.get(k.index) }} />
            )}
            {overridden?.has(k.index) && <span className="kr-mark is-override" />}
          </button>
        );
      })}
    </fieldset>
  );
};
