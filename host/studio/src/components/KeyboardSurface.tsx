import type React from "react";
import { useEffect, useState } from "react";
import { type HardwareAttachmentBay, v2Client } from "../api/v2Client";

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
  /** When true, renders the live GPU-composited KeyGlass keyboard frame underneath sculpted refractive glass keycaps! */
  showLivePreview?: boolean;
  /** Optional refresh token to re-capture a live preview frame when configs change. */
  previewTick?: number;
  /** Renders per-key labels/colors when `showLivePreview` is false. */
  renderKey?: (key: SurfaceKeyGeom) => KeyCapVisual;
  /** Optional compact top-bar strip text when `showLivePreview` is false. */
  topBarSummary?: string;
  /** Optional physical hardware attachments in the 4 top-bar bays; auto-loaded if omitted. */
  attachments?: HardwareAttachmentBay[];
}

const DISPLAY_W = 1920;
const DISPLAY_H = 1080;

const DEFAULT_BAYS: HardwareAttachmentBay[] = [
  { pos: 0, span: 1, kind: "clear", opaque: false, label: "Clear Window" },
  { pos: 1, span: 1, kind: "clear", opaque: false, label: "Clear Window" },
  { pos: 2, span: 1, kind: "clear", opaque: false, label: "Clear Window" },
  { pos: 3, span: 1, kind: "clear", opaque: false, label: "Clear Window" },
];

const AttachmentVisual: React.FC<{ bay: HardwareAttachmentBay }> = ({ bay }) => {
  const leftPct = (bay.pos / 4) * 100;
  const widthPct = (bay.span / 4) * 100;

  if (bay.kind === "clear") {
    return (
      <div
        className="kb-att-bay kb-att-clear"
        style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
        title="Clear Acrylic Window"
      />
    );
  }

  return (
    <div
      className={`kb-att-bay kb-att-opaque kb-att-${bay.kind}`}
      style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
      title={`${bay.label ?? bay.kind} (Physical Attachment)`}
    >
      {bay.kind === "dial" && (
        <div className="kb-hw-dial">
          <div className="kb-hw-dial-cap">
            <span className="kb-hw-dial-pip" />
          </div>
        </div>
      )}
      {bay.kind === "triple_dial" && (
        <div className="kb-hw-triple-dial">
          {[0, 1, 2].map((i) => (
            <div key={i} className="kb-hw-dial kb-hw-dial-sm">
              <div className="kb-hw-dial-cap">
                <span className="kb-hw-dial-pip" />
              </div>
            </div>
          ))}
        </div>
      )}
      {bay.kind === "slider" && (
        <div className="kb-hw-slider">
          <div className="kb-hw-slider-track">
            <div className="kb-hw-slider-thumb" />
          </div>
        </div>
      )}
      {bay.kind === "macro_4" && (
        <div className="kb-hw-macro4">
          {[1, 2, 3, 4].map((i) => (
            <div key={i} className="kb-hw-macro-cap">
              M{i}
            </div>
          ))}
        </div>
      )}
      {bay.kind === "blank" && <span className="kb-hw-blank-label">Cover</span>}
    </div>
  );
};

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
  attachments: propAttachments,
}) => {
  const [frameSrc, setFrameSrc] = useState<string | null>(null);
  const [loadedBays, setLoadedBays] = useState<HardwareAttachmentBay[]>(DEFAULT_BAYS);

  useEffect(() => {
    if (propAttachments) return;
    let alive = true;
    void v2Client
      .getDisplayAttachments()
      .then((res) => {
        if (alive && Array.isArray(res.bays) && res.bays.length > 0) {
          setLoadedBays(res.bays);
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [propAttachments, previewTick]);

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

  const bays = propAttachments ?? loadedBays;

  return (
    <div className="kb-surface-frame">
      <div className="kb-surface-chassis-sheen" aria-hidden="true" />
      <div
        className="kb-surface-canvas"
        style={{ aspectRatio: `${DISPLAY_W} / ${DISPLAY_H}` }}
      >
        {showLivePreview && frameSrc && (
          <img
            src={frameSrc}
            alt="Live KeyGlass Keyboard Display"
            className="kb-surface-preview-img"
            draggable={false}
          />
        )}

        {/* Clickable Top Bar Strip + Physical Hardware Attachments */}
        <button
          type="button"
          className={`kb-surface-topbar ${topBarSelected ? "is-selected" : ""} ${showLivePreview ? "is-overlay" : ""}`}
          onClick={() => onSelectTopBar?.()}
          title="Click to configure Top Bar Widgets & Physical Hardware Attachments"
        >
          <div className="kb-att-strip" aria-hidden="true">
            {bays.map((bay, idx) => (
              <AttachmentVisual key={`${bay.pos}-${bay.kind}-${idx}`} bay={bay} />
            ))}
          </div>
          {!showLivePreview && (
            <span className="kb-surface-topbar-label">
              {topBarSummary || "Top Bar Display & Hardware Attachments"}
            </span>
          )}
        </button>

        {/* Interactive 85-Key Sculpted Clear Glass Keycaps */}
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
              className={`kb-key ${isSel ? "is-selected" : ""} ${showLivePreview ? "is-glass-cap" : ""} ${vis?.muted ? "is-muted" : ""}`}
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
              {showLivePreview && <span className="kb-glass-lens" aria-hidden="true" />}
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
