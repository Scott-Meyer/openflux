import type React from "react";
import { useEffect, useState } from "react";
import { v2Client } from "../api/v2Client";

export interface SurfaceKeyGeom {
  index: number;
  row: number;
  col: number;
  center_x: number;
  center_y: number;
  rect: { x: number; y: number; w: number; h: number };
}

export interface KeyCapVisual {
  label: string;
  sub?: string;
  color?: string;
  bg?: string;
  border?: string;
  dotColor?: string;
  secondaryDotColor?: string;
  muted?: boolean;
  title?: string;
}

interface KeyboardSurfaceProps {
  keys: SurfaceKeyGeom[];
  selectedKey: number | null;
  topBarSelected?: boolean;
  onSelectKey?: (keyIndex: number) => void;
  onSelectTopBar?: () => void;
  /** When true, renders the live GPU-composited OpenFlux keyboard frame underneath interactive hit zones! */
  showLivePreview?: boolean;
  /** Optional refresh token to re-capture a live preview frame when configs change. */
  previewTick?: number;
  /** Renders per-key labels/colors when `showLivePreview` is false (or overlaid gently when editing). */
  renderKey?: (key: SurfaceKeyGeom) => KeyCapVisual;
  /** Optional compact top-bar strip text when `showLivePreview` is false. */
  topBarSummary?: string;
}

const DISPLAY_W = 1920;
const DISPLAY_H = 1080;

export const KeyboardSurface: React.FC<KeyboardSurfaceProps> = ({
  keys,
  selectedKey,
  topBarSelected = false,
  onSelectKey,
  onSelectTopBar,
  showLivePreview = false,
  previewTick = 0,
  renderKey,
  topBarSummary,
}) => {
  const [frameSrc, setFrameSrc] = useState<string | null>(null);

  useEffect(() => {
    if (!showLivePreview) return;
    let alive = true;
    const capture = () => {
      void v2Client
        .renderPreviewFrame(2, 16.667, 960, 540)
        .then((res) => {
          if (alive && res.png_base64) {
            setFrameSrc(`data:image/png;base64,${res.png_base64}`);
          }
        })
        .catch(() => {});
    };
    capture();
    const id = window.setInterval(capture, 2500);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [showLivePreview, previewTick]);

  return (
    <div className="kb-surface-frame">
      <div
        className="kb-surface-canvas"
        style={{ aspectRatio: `${DISPLAY_W} / ${DISPLAY_H}` }}
      >
        {showLivePreview && frameSrc && (
          <img
            src={frameSrc}
            alt="Live OpenFlux Keyboard Display"
            className="kb-surface-preview-img"
            draggable={false}
          />
        )}

        {/* Clickable Top Bar Strip (y = 0..252 / 1080) */}
        <button
          type="button"
          className={`kb-surface-topbar ${topBarSelected ? "is-selected" : ""} ${showLivePreview ? "is-overlay" : ""}`}
          onClick={() => onSelectTopBar?.()}
          title="Click to inspect or arrange the Top Bar Dock"
        >
          {!showLivePreview && (
            <span className="kb-surface-topbar-label">
              {topBarSummary || "Top Bar Display Strip"}
            </span>
          )}
        </button>

        {/* Interactive 85-Key Flux Matrix */}
        {keys.map((k) => {
          const isSel = selectedKey === k.index;
          const vis = renderKey?.(k);
          const leftPct = (k.rect.x / DISPLAY_W) * 100;
          const topPct = (k.rect.y / DISPLAY_H) * 100;
          const wPct = (k.rect.w / DISPLAY_W) * 100;
          const hPct = (k.rect.h / DISPLAY_H) * 100;

          return (
            <button
              key={k.index}
              type="button"
              onClick={() => onSelectKey?.(k.index)}
              title={vis?.title ?? `Key ${k.index} (R${k.row} C${k.col})`}
              className={`kb-key ${isSel ? "is-selected" : ""} ${showLivePreview ? "is-live-overlay" : ""} ${vis?.muted ? "is-muted" : ""}`}
              style={{
                left: `calc(${leftPct}% + 2px)`,
                top: `calc(${topPct}% + 2px)`,
                width: `calc(${wPct}% - 4px)`,
                height: `calc(${hPct}% - 4px)`,
                color: vis?.color,
                background: showLivePreview ? undefined : vis?.bg,
                borderColor: isSel ? undefined : vis?.border,
              }}
            >
              {vis?.dotColor && (
                <span
                  className="kb-key-dot"
                  style={{
                    background: vis.dotColor,
                    right: vis.secondaryDotColor ? 11 : 4,
                  }}
                />
              )}
              {vis?.secondaryDotColor && (
                <span
                  className="kb-key-dot"
                  style={{ background: vis.secondaryDotColor, right: 4 }}
                />
              )}
              {!showLivePreview && vis && (
                <>
                  <span className="kb-key-main">{vis.label}</span>
                  {vis.sub && <span className="kb-key-sub">{vis.sub}</span>}
                </>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
};
