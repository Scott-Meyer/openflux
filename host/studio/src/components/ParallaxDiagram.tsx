import type React from "react";
import type { SettingsConfig } from "../types/openflux";

const FRAME_W_UNITS = 309.31;
const FRAME_H_UNITS = 121.82;
const PHYS_W_MM = 307.0;
const PHYS_H_MM = 123.0;
const KEYCAP_THICKNESS_MM = 9.0;
const REFRACTIVE_INDEX = 1.51;
const MAX_OFFSET_PX = 28.0;
const DISPLAY_W = 1920.0;
const DISPLAY_H = 1080.0;

/**
 * Parallax offset of the keyboard's center key: light refracts through a 9 mm keycap
 * (Snell's law) toward the configured eye position. Uses the same model as
 * `compute_key_parallax` in `keyboard/display/src/layout.rs`, so the diagram matches
 * what the keyboard renders.
 */
export function computeCenterKeyParallax(settings: SettingsConfig): {
  standDeg: number;
  dxPx: number;
  dyPx: number;
} {
  const standDeg = settings.footStandMode === "high" ? 8 : settings.footStandMode === "low" ? 4 : 0;
  const baseDx = settings.displayOffsetX || 0;
  const baseDy = settings.displayOffsetY || 0;
  if (!settings.parallaxActive || (settings.eyeHeight || 0) <= 10.0) {
    return { standDeg, dxPx: baseDx, dyPx: baseDy };
  }

  const pxPerUnit = DISPLAY_W / FRAME_W_UNITS;
  const keyXUnits = 0.0;
  const keyYFromFrontUnits = Math.min(FRAME_H_UNITS, Math.max(0.0, (DISPLAY_H - 540.0) / pxPerUnit));
  const keyXMm = keyXUnits * (PHYS_W_MM / FRAME_W_UNITS);
  const keyYFromFrontMm = keyYFromFrontUnits * (PHYS_H_MM / FRAME_H_UNITS);

  const tiltRad = (standDeg * Math.PI) / 180.0;
  const c = Math.cos(tiltRad);
  const s = Math.sin(tiltRad);
  const eyeXMm = settings.eyeXOffset || 0;
  const eyeDistFrontMm = settings.eyeDistanceFromFront || 225;
  const eyeHeightMm = settings.eyeHeight || 425;

  const relY = eyeDistFrontMm * c + eyeHeightMm * s;
  const relZ = eyeHeightMm * c - eyeDistFrontMm * s;
  if (relZ <= 10.0) {
    return { standDeg, dxPx: baseDx, dyPx: baseDy };
  }

  const xDist = keyXMm - eyeXMm;
  const yDist = keyYFromFrontMm + relY;
  const rHoriz = Math.hypot(xDist, yDist);
  if (rHoriz < 1e-6) {
    return { standDeg, dxPx: baseDx, dyPx: baseDy };
  }

  const r3d = Math.hypot(rHoriz, relZ);
  const sinAir = rHoriz / r3d;
  const sinKeycap = Math.min(0.999, Math.max(-0.999, sinAir / REFRACTIVE_INDEX));
  const tanKeycap = sinKeycap / Math.sqrt(1.0 - sinKeycap * sinKeycap);
  const tanAir = rHoriz / relZ;

  const shiftXMm = (tanAir - tanKeycap) * KEYCAP_THICKNESS_MM * (xDist / rHoriz);
  const shiftYMm = -(tanKeycap * KEYCAP_THICKNESS_MM) * (yDist / rHoriz);

  let dx = shiftXMm * (DISPLAY_W / PHYS_W_MM);
  let dy = shiftYMm * pxPerUnit * (FRAME_H_UNITS / PHYS_H_MM);
  const mag = Math.hypot(dx, dy);
  if (mag > MAX_OFFSET_PX) {
    const clamp = MAX_OFFSET_PX / mag;
    dx *= clamp;
    dy *= clamp;
  }
  return { standDeg, dxPx: baseDx + dx, dyPx: baseDy + dy };
}

interface ParallaxDiagramProps {
  settings: SettingsConfig;
}

export const ParallaxDiagram: React.FC<ParallaxDiagramProps> = ({ settings }) => {
  const eyeH = Math.max(150, Math.min(800, settings.eyeHeight || 425));
  const eyeD = Math.max(50, Math.min(600, settings.eyeDistanceFromFront || 225));
  const { standDeg, dxPx, dyPx } = computeCenterKeyParallax(settings);

  const eyeSvgX = Math.max(36, Math.min(280, 120 - (eyeD - 225) * 0.16));
  const eyeSvgY = Math.max(18, Math.min(100, 118 - (eyeH / 800) * 90));
  const kbdY = 148;

  return (
    <figure className="parallax-diagram-box" aria-label="Optical Parallax Geometry Diagram">
      <div className="toolbar-between" style={{ marginBottom: 6 }}>
        <strong style={{ fontSize: 12 }}>Center Key Parallax</strong>
        <span className="pill" style={{ whiteSpace: "nowrap" }}>
          {settings.parallaxActive
            ? `${standDeg}° · Δ(${dxPx.toFixed(1)}, ${dyPx.toFixed(1)}) px`
            : `Off · (${dxPx.toFixed(1)}, ${dyPx.toFixed(1)}) px`}
        </span>
      </div>
      <svg viewBox="0 0 360 176" className="parallax-svg" role="img" aria-label="Side profile parallax geometry">
        <defs>
          <linearGradient id="lcdGlow" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="#38bdf8" stopOpacity="0.25" />
            <stop offset="50%" stopColor="#81edff" stopOpacity="0.7" />
            <stop offset="100%" stopColor="#38bdf8" stopOpacity="0.25" />
          </linearGradient>
        </defs>
        <rect x="0" y="0" width="360" height="176" rx="8" fill="#090d16" />
        <line x1="20" y1="162" x2="340" y2="162" stroke="#24304a" strokeWidth="1.5" />

        <g transform={`rotate(${-standDeg}, 175, ${kbdY + 8})`}>
          <rect x="165" y={kbdY - 6} width="142" height="14" rx="3" fill="#1b2438" stroke="#3c527a" />
          <line x1="173" y1={kbdY + 3} x2="299" y2={kbdY + 3} stroke="url(#lcdGlow)" strokeWidth="2.5" />
          <line
            x1="173"
            y1={kbdY - 5}
            x2="299"
            y2={kbdY - 5}
            stroke="#9fb3d9"
            strokeWidth="1.2"
            strokeDasharray="3 2"
          />
          <text x="310" y={kbdY - 2} fill="#8ea1c4" fontSize="8">
            Keycaps
          </text>
          <text x="310" y={kbdY + 8} fill="#66ccff" fontSize="8">
            LCD
          </text>
        </g>

        <circle cx={eyeSvgX} cy={eyeSvgY} r="7" fill="#0f172a" stroke="#66ccff" strokeWidth="1.8" />
        <circle cx={eyeSvgX} cy={eyeSvgY} r="2.5" fill="#81edff" />
        <text x={eyeSvgX + 11} y={eyeSvgY + 3} fill="#dce6f7" fontSize="9">
          Eye ({eyeH}mm H, {eyeD}mm F)
        </text>

        <line
          x1={eyeSvgX}
          y1={eyeSvgY}
          x2="188"
          y2={kbdY - 4}
          stroke="#66ccff"
          strokeWidth="1"
          strokeDasharray="3 3"
          opacity="0.7"
        />
        <line
          x1={eyeSvgX}
          y1={eyeSvgY}
          x2="292"
          y2={kbdY - 4}
          stroke="#66ccff"
          strokeWidth="1"
          strokeDasharray="3 3"
          opacity="0.7"
        />
        <line x1={eyeSvgX} y1={eyeSvgY} x2="240" y2={kbdY + 3} stroke="#81edff" strokeWidth="1.4" />
        <circle cx="240" cy={kbdY + 3} r="2.6" fill="#81edff" />
      </svg>
    </figure>
  );
};
