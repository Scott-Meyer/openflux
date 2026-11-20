import type React from "react";
import { useEffect, useRef, useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import type { OpenFluxConfig, PreviewKeyRect } from "../types/openflux";
import { humanKeyName } from "../types/openflux";

interface LivePreviewCanvasProps {
  client: DaemonClient;
  config: OpenFluxConfig;
  syncedRevision: number;
  activeLayer: number;
  selectedKeyIndex: number | null;
  /** The key rectangles of the current layout, in display pixels, for clicking keys. */
  rects: PreviewKeyRect[];
  onSelectKey: (idx: number) => void;
}

/**
 * Mosaic tile types whose content changes over time (the display's `TileType`), so a preview
 * showing one keeps rendering. Display preferences (6) are static.
 */
const LIVE_TILE_TYPES: ReadonlySet<number> = new Set([
  2, // digital clock
  3, // telemetry
  4, // device info
  5, // sensor readout
  7, // hardware graph
  8, // hardware status
  9, // keyboard state
  10, // analog clock
  11, // flip clock
  12, // minimal clock
  13, // Pomodoro
  14, // world clock
  15, // weather
  16, // active media
  17, // virtual pet
  18, // generic
]);

export const LivePreviewCanvas: React.FC<LivePreviewCanvasProps> = ({
  client,
  config,
  syncedRevision,
  activeLayer,
  selectedKeyIndex,
  rects,
  onSelectKey,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const inFlightRef = useRef(false);
  const seqRef = useRef(1);
  const [pressedKeys, setPressedKeys] = useState<number[]>([]);
  const [suspended, setSuspended] = useState(false);
  const [renderError, setRenderError] = useState<string | null>(null);

  const latestStateRef = useRef({ activeLayer, pressedKeys, suspended, syncedRevision, config });
  latestStateRef.current = { activeLayer, pressedKeys, suspended, syncedRevision, config };
  const dirtyWhileInFlightRef = useRef(false);
  const animUntilRef = useRef(0);
  const kickRenderRef = useRef<(() => void) | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: these inputs change the rendered frame, so each change requests a render; the render loop reads them through latestStateRef.
  useEffect(() => {
    if (pressedKeys.length > 0) {
      animUntilRef.current = performance.now() + 600;
    } else if (animUntilRef.current > 0) {
      animUntilRef.current = Math.max(animUntilRef.current, performance.now() + 550);
    }
    if (inFlightRef.current) {
      dirtyWhileInFlightRef.current = true;
    } else {
      kickRenderRef.current?.();
    }
  }, [config, syncedRevision, activeLayer, pressedKeys, suspended]);

  useEffect(() => {
    let cancelled = false;
    let timerId: number | undefined;

    const scheduleNext = (delayMs: number) => {
      if (cancelled) return;
      if (timerId !== undefined) window.clearTimeout(timerId);
      timerId = window.setTimeout(() => {
        timerId = undefined;
        void renderFrame();
      }, delayMs);
    };

    const nextIdleDelayMs = (snap: typeof latestStateRef.current): number | null => {
      if (snap.suspended) return null;
      const sid = snap.config.appearance.selectedId;
      const activeTheme =
        snap.config.appearance.customThemes.find((t) => t.id === sid) ?? snap.config.appearance.defaultTheme;
      const isVideo = /\.(mp4|mov|mkv|webm|avi|gif|frag)$/i.test(activeTheme?.backgroundImagePath ?? "");
      if (isVideo || snap.pressedKeys.length > 0 || performance.now() < animUntilRef.current) {
        return 100;
      }
      const hasLiveTile = snap.config.mosaic.tileTypes.some((t) => LIVE_TILE_TYPES.has(t));
      return hasLiveTile ? 1000 : null;
    };

    const renderFrame = async () => {
      if (cancelled) return;
      if (inFlightRef.current) {
        dirtyWhileInFlightRef.current = true;
        scheduleNext(25);
        return;
      }
      if (typeof document !== "undefined" && document.hidden) {
        scheduleNext(500);
        return;
      }
      inFlightRef.current = true;
      dirtyWhileInFlightRef.current = false;
      const t0 = performance.now();
      const snap = latestStateRef.current;
      try {
        const bytes = new Uint8Array(32);
        for (const k of snap.pressedKeys) {
          if (k >= 0 && k < 256) bytes[k >> 3] |= 1 << (k & 7);
        }
        const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
        const seq = seqRef.current++;
        const rgba = await client.renderPreviewRgba({
          suspended: snap.suspended,
          halfRes: true,
          config: snap.config,
          liveJson: {
            type: "key_state",
            seq,
            active_layer_bitfield: 1 << snap.activeLayer,
            key_bitmap: hex,
          },
        });
        if (cancelled) return;
        setRenderError(null);

        const cvs = canvasRef.current;
        const dims: [number, number] | null =
          rgba.length === 960 * 540 * 4 ? [960, 540] : rgba.length === 1920 * 1080 * 4 ? [1920, 1080] : null;
        if (cvs && dims) {
          const [rw, rh] = dims;
          if (cvs.width !== rw || cvs.height !== rh) {
            cvs.width = rw;
            cvs.height = rh;
          }
          const ctx = cvs.getContext("2d");
          if (ctx) {
            const view = new Uint8ClampedArray(rgba.buffer as ArrayBuffer, rgba.byteOffset, rgba.byteLength);
            const imgData = new ImageData(view, rw, rh);
            ctx.putImageData(imgData, 0, 0);
          }
        }
      } catch (err) {
        if (!cancelled) setRenderError(String(err));
      } finally {
        inFlightRef.current = false;
        if (!cancelled) {
          if (dirtyWhileInFlightRef.current) {
            scheduleNext(0);
          } else {
            const delay = nextIdleDelayMs(latestStateRef.current);
            if (delay !== null) {
              const elapsed = performance.now() - t0;
              scheduleNext(Math.max(0, delay - elapsed));
            }
          }
        }
      }
    };

    kickRenderRef.current = () => {
      if (!cancelled && !inFlightRef.current) {
        if (timerId !== undefined) {
          window.clearTimeout(timerId);
          timerId = undefined;
        }
        void renderFrame();
      }
    };

    void renderFrame();

    return () => {
      cancelled = true;
      kickRenderRef.current = null;
      if (timerId !== undefined) window.clearTimeout(timerId);
    };
  }, [client]);

  const hitTestKey = (e: React.MouseEvent<HTMLCanvasElement>): PreviewKeyRect | undefined => {
    const cvs = canvasRef.current;
    if (!cvs) return undefined;
    const box = cvs.getBoundingClientRect();
    const px = ((e.clientX - box.left) / box.width) * 1920;
    const py = ((e.clientY - box.top) / box.height) * 1080;
    return rects.find((r) => px >= r.x && px <= r.x + r.w && py >= r.y && py <= r.y + r.h);
  };

  const handleCanvasMouseDown = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const hit = hitTestKey(e);
    if (hit) {
      onSelectKey(hit.keyIndex);
      setPressedKeys([hit.keyIndex]);
    }
  };

  const releaseHeldKeys = () => {
    setPressedKeys((prev) => (prev.length > 0 ? [] : prev));
  };

  const selectedLabel = selectedKeyIndex !== null ? humanKeyName(config, selectedKeyIndex) : null;

  return (
    <section className="preview-stage" aria-label="Live Keyboard Display Preview">
      <div className="preview-toolbar">
        <div className="preview-title">
          <span className="pill">Keyboard Display · 1920×1080</span>
          <span className="muted">
            {config.keymap.selectedFrameLayoutType.toUpperCase()} ({rects.length} keys) · Layer {activeLayer}
          </span>
          {selectedLabel && <span className="pill">Selected: {selectedLabel}</span>}
        </div>
        <div className="preview-actions">
          <button
            type="button"
            className={`btn-secondary ${pressedKeys.length > 0 ? "active" : ""}`}
            disabled={selectedKeyIndex === null}
            onMouseDown={() => {
              if (selectedKeyIndex !== null) setPressedKeys([selectedKeyIndex]);
            }}
            onMouseUp={releaseHeldKeys}
            onMouseLeave={releaseHeldKeys}
            onKeyDown={(e) => {
              if ((e.key === " " || e.key === "Enter") && selectedKeyIndex !== null) {
                setPressedKeys([selectedKeyIndex]);
              }
            }}
            onKeyUp={releaseHeldKeys}
          >
            {pressedKeys.length > 0 ? "Holding Key..." : "Test Key Press"}
          </button>
          <button
            type="button"
            className={`btn-secondary ${suspended ? "active" : ""}`}
            onClick={() => setSuspended((s) => !s)}
          >
            {suspended ? "Wake Display" : "Preview Sleep"}
          </button>
        </div>
      </div>

      <div className="preview-viewport">
        <canvas
          ref={canvasRef}
          width={1920}
          height={1080}
          onMouseDown={handleCanvasMouseDown}
          onMouseUp={releaseHeldKeys}
          onMouseLeave={releaseHeldKeys}
          className="preview-canvas"
          title="Click or hold any key to select & test its glow effect"
        />
      </div>
      {renderError && <p className="error-text">Preview unavailable: {renderError}</p>}
    </section>
  );
};
