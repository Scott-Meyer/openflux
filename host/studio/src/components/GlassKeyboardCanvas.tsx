import type React from "react";
import { useEffect, useRef, useState } from "react";
import { v2Client } from "../api/v2Client";
import type { SurfaceKeyGeom } from "./KeyboardSurface";

interface GlassKeyboardCanvasProps {
  keys: SurfaceKeyGeom[];
  selectedKey: number | null;
  hoveredKey: number | null;
  previewTick: number;
}

const DISPLAY_W = 1920;
const DISPLAY_H = 1080;
const MAP_W = 1920;
const MAP_H = 1080;

const VERT_SRC = `#version 300 es
in vec2 a_pos;
out vec2 v_uv;
void main() {
  v_uv = vec2(a_pos.x * 0.5 + 0.5, 1.0 - (a_pos.y * 0.5 + 0.5));
  gl_Position = vec4(a_pos, 0.0, 1.0);
}
`;

const FRAG_SRC = `#version 300 es
precision highp float;

in vec2 v_uv;
out vec4 out_color;

uniform sampler2D u_video;      // Live 60 FPS background video (or fallback background)
uniform sampler2D u_overlay;    // Crisp transparent 2D overlay (key legends, top bar widgets, toasts)
uniform sampler2D u_glass_map;  // 1920x1080 continuous SDF (B), key-local UV (RG), maglev post/corner mask (A)
uniform float u_has_video;
uniform float u_has_overlay;
uniform float u_bg_dim;
uniform float u_top_fade;
uniform float u_time;
uniform vec4 u_sel_rect;        // normalized (x0, y0, x1, y1) of selected key
uniform vec4 u_hov_rect;        // normalized (x0, y0, x1, y1) of hovered key

vec3 sample_bg(vec2 uv) {
  vec2 suv = clamp(uv, vec2(0.001), vec2(0.999));
  vec3 bg = vec3(0.03, 0.05, 0.09);
  if (u_has_video > 0.5) {
    bg = texture(u_video, suv).rgb;
    // Slight vibrant IPS backlight boost matching real Flux panel brightness
    bg *= (1.0 - clamp(u_bg_dim * 0.72, 0.0, 0.72));
  }
  if (u_top_fade > 0.001 && suv.y < 0.255) {
    float fade_t = 1.0 - smoothstep(0.22, 0.255, suv.y);
    bg *= (1.0 - u_top_fade * fade_t);
  }
  return bg;
}

vec4 sample_ov(vec2 uv) {
  if (u_has_overlay < 0.5) return vec4(0.0);
  return texture(u_overlay, clamp(uv, vec2(0.001), vec2(0.999)));
}

vec3 sample_full(vec2 uv) {
  vec3 bg = sample_bg(uv);
  vec4 ov = sample_ov(uv);
  return mix(bg, ov.rgb, ov.a);
}

float in_rect(vec2 uv, vec4 r) {
  if (r.z <= r.x || r.w <= r.y) return 0.0;
  return step(r.x, uv.x) * step(uv.x, r.z) * step(r.y, uv.y) * step(uv.y, r.w);
}

void main() {
  // 1. Top Display Strip + Bright Metallic Horizontal Divider Rail (y = 0..268 in 1080p)
  float y_px = v_uv.y * 1080.0;
  if (y_px < 268.0) {
    vec3 scr = sample_full(v_uv);
    // Crisp clear acrylic top window with subtle top sheen + glossy lower internal-reflection lip
    float top_sheen = smoothstep(70.0, 0.0, y_px) * 0.045;
    // Lower acrylic bottom-edge internal-reflection band (like the bright strip just above the divider in photo)
    float lower_lip = smoothstep(14.0, 0.0, abs(y_px - 246.0)) * 0.14;
    // Horizontal brushed metallic/chrome divider rail at y = 258..264
    float bar_trench = smoothstep(8.5, 3.0, abs(y_px - 260.0));
    float bar_metal = smoothstep(3.2, 0.6, abs(y_px - 260.0));
    float bar_spec = smoothstep(1.2, 0.0, abs(y_px - 259.4));

    vec3 col = scr * (1.04 - bar_trench * 0.80);
    col += vec3(0.92, 0.94, 0.96) * (top_sheen + lower_lip * 0.7);
    // Warm-silver anodized/chrome divider bar matching real hardware
    vec3 chrome = mix(vec3(0.28, 0.27, 0.25), vec3(0.84, 0.82, 0.76), bar_spec);
    col = mix(col, chrome, bar_metal * 0.92);
    out_color = vec4(clamp(col, 0.0, 1.0), 1.0);
    return;
  }

  // 2. Keycap Deck: sample continuous 1920x1080 SDF + key-local coordinates
  vec4 gm = texture(u_glass_map, v_uv);
  vec2 ku = gm.rg * 2.0 - 1.0;        // key-local (-1..1, -1..1), 0 outside keys
  // Decode exact signed distance in 1080p pixels: > 0 inside keycap, < 0 outside keycap
  float dist_in_px = (gm.b - 0.5) * 42.0;
  float post_mask = gm.a;             // >0.5 = dark vertical maglev guide post inside left/right wall

  // Outward unit normal approximation for wall refraction (smooth everywhere via key-local coords)
  vec2 abs_ku = abs(ku);
  vec2 wall_n = normalize(vec2(
    sign(ku.x) * smoothstep(0.45, 0.95, abs_ku.x),
    sign(ku.y) * smoothstep(0.45, 0.95, abs_ku.y)
  ) + vec2(1e-4, 0.0));
  float corner_fac = smoothstep(0.68, 0.96, abs_ku.x) * smoothstep(0.68, 0.96, abs_ku.y);

  // Wall geometry in exact 1080p pixels (calibrated from 1:1 macro photo of physical keycaps)
  // Side walls are slightly thicker (~9.5px) to house the maglev guide posts; top/bottom rims are
  // sleek (~6.2px) so dual-shift symbols (! @ # $ %) & icons stay 100% crisp and unclipped!
  float wall_w = mix(6.2, 9.5, abs(wall_n.x));

  vec3 col;
  if (dist_in_px <= 0.0) {
    // 2A. Outside keycaps: BRIGHT, CLEAR acrylic apron & inter-row spaces!
    // Narrow 2.5px structural optical shadow hugging the outer keycap base
    float gap_px = -dist_in_px;
    float edge_shadow = smoothstep(2.8, 0.0, gap_px);
    vec3 bg = sample_bg(v_uv);
    float frame_glint = smoothstep(5.0, 2.2, gap_px) * smoothstep(0.6, 2.2, gap_px) * 0.16;
    col = bg * mix(1.04, 0.34, edge_shadow) + vec3(0.88, 0.92, 0.96) * frame_glint;
  } else if (dist_in_px < wall_w) {
    // 2B. Thick Vertical Clear Acrylic Keycap Wall (Total Internal Reflection + Optical Scramble)
    // Optically scrambles ONLY background ocean (sample_bg — never ghosts key legends!)
    float wt = dist_in_px / wall_w; // 0 = outer edge, 1 = inner crystal window edge
    vec2 perp = vec2(-wall_n.y, wall_n.x);

    // Multi-bounce crisp internal acrylic refraction (obscures direct reading through thick edges)
    vec3 s1 = sample_bg(v_uv - wall_n * 0.018 + perp * 0.006);
    vec3 s2 = sample_bg(v_uv + wall_n * 0.022 - perp * 0.006);
    vec3 s3 = sample_bg(v_uv - wall_n * 0.008 - perp * 0.010);
    vec3 s_mix = s1 * 0.45 + s2 * 0.35 + s3 * 0.20;
    float luma = dot(s_mix, vec3(0.299, 0.587, 0.114));
    vec3 piped = mix(s_mix, vec3(luma), 0.16) * 1.15;

    // Physical acrylic skirt shading: crisp dark base edge -> bright internal glass band -> subtle inner lip
    float base_dark = smoothstep(0.24, 0.0, wt);
    float mid_band = smoothstep(0.14, 0.48, wt) * smoothstep(0.95, 0.58, wt);
    float inner_lip = smoothstep(0.76, 0.98, wt);

    // Overhead directional specular (horizontal top/bottom bevels & 45-deg corners catch bright silver light)
    float horiz_catch = max(0.0, -wall_n.y) * 0.28 + max(0.0, wall_n.y) * 0.20;
    float vert_catch = abs(wall_n.x) * 0.10;

    col = piped * mix(0.55, 1.06, smoothstep(0.0, 0.35, wt));
    col *= (1.0 - base_dark * 0.42);
    col += vec3(0.90, 0.94, 0.98) * (
      mid_band * (0.14 + horiz_catch + vert_catch) +
      inner_lip * (0.14 + horiz_catch * 0.5) +
      corner_fac * 0.34
    );

    // Signature Flux Maglev Side-Wall Guide Posts (crisp dark vertical blocks inside L/R acrylic walls!)
    if (post_mask > 0.02) {
      vec3 post_col = vec3(0.04, 0.045, 0.055) + s_mix * 0.08;
      col = mix(col, post_col, clamp(post_mask * 1.05, 0.0, 0.94));
    }

    // Keep any legend/icon glyphs that approach the inner edge (wt > 0.65) crisp and unclipped!
    if (wt > 0.65) {
      vec4 ov_edge = sample_ov(v_uv);
      col = mix(col, ov_edge.rgb, ov_edge.a * smoothstep(0.65, 0.95, wt));
    }
  } else {
    // 2C. Uniform Middle-Key Convex-Bottom Fisheye Crystal Top Window
    // Gently pulls in ~6% extra surrounding screen uniformly across every keycap so the brain
    // stitches one complete, vivid, jewel-like continuous image across the whole board!
    float r2 = dot(ku, ku);
    vec2 fisheye_shift = ku * (0.0042 + 0.0038 * r2);
    vec2 bg_uv = v_uv + fisheye_shift;
    vec2 ov_uv = v_uv + ku * 0.0010;

    vec3 bg = sample_bg(bg_uv);
    vec4 ov = sample_ov(ov_uv);
    vec3 scr = mix(bg, ov.rgb, ov.a);

    // Crystal-clear zero-haze transmission + ultra-fine inner bevel catch-light
    float inner_edge = smoothstep(wall_w + 2.8, wall_w, dist_in_px);
    float micro_sheen = smoothstep(0.85, -0.85, ku.y) * 0.022;
    col = scr * 1.06 + vec3(0.92, 0.96, 1.0) * (micro_sheen + inner_edge * 0.09);
  }

  // Interactive 3D internal acrylic edge glow on Hovered / Selected key
  float is_hov = in_rect(v_uv, u_hov_rect);
  float is_sel = in_rect(v_uv, u_sel_rect);
  if (is_hov > 0.5 && dist_in_px > 0.0) {
    float boost = (dist_in_px < wall_w) ? 0.40 : 0.06;
    col += vec3(0.62, 0.52, 1.0) * boost;
  }
  if (is_sel > 0.5 && dist_in_px > 0.0) {
    float boost = (dist_in_px < wall_w) ? 0.62 : 0.10;
    col += vec3(0.45, 0.72, 1.0) * boost;
  }

  // Subtle physical outer bezel edge vignette
  vec2 edge = min(v_uv, 1.0 - v_uv);
  float vig = smoothstep(0.0, 0.010, edge.x) * smoothstep(0.0, 0.014, edge.y);
  col *= mix(0.80, 1.0, vig);

  out_color = vec4(clamp(col, 0.0, 1.0), 1.0);
}
`;

/** Precomputes a native 1920x1080 continuous SDF + key-local UV + maglev guide-post map. */
function buildGlassNormalMap(keys: SurfaceKeyGeom[]): Uint8Array {
  const data = new Uint8Array(MAP_W * MAP_H * 4);
  const sx = MAP_W / DISPLAY_W;
  const sy = MAP_H / DISPLAY_H;

  // Initialize every pixel to far-outside clear apron (ku=(0,0), dist_in=-21px -> B=0, post=0)
  for (let i = 0; i < MAP_W * MAP_H; i++) {
    const o = i * 4;
    data[o] = 128;
    data[o + 1] = 128;
    data[o + 2] = 0;
    data[o + 3] = 0;
  }

  const topH = Math.round(268 * sy);

  // Rasterize each of the 85 Flux clear acrylic keycaps at native 1920x1080 sharpness!
  for (const k of keys) {
    const padOut = 10;
    const x0 = Math.max(0, Math.floor(k.rect.x * sx) - padOut);
    const y0 = Math.max(topH, Math.floor(k.rect.y * sy) - padOut);
    const x1 = Math.min(MAP_W - 1, Math.ceil((k.rect.x + k.rect.w) * sx) + padOut);
    const y1 = Math.min(MAP_H - 1, Math.ceil((k.rect.y + k.rect.h) * sy) + padOut);

    const cx = (k.rect.x + k.rect.w * 0.5) * sx;
    const cy = (k.rect.y + k.rect.h * 0.5) * sy;
    const hw = Math.max(8, (k.rect.w * 0.5 - 1.5) * sx);
    const hh = Math.max(8, (k.rect.h * 0.5 - 1.5) * sy);
    const cornerR = 6.0;

    for (let py = y0; py <= y1; py++) {
      for (let px = x0; px <= x1; px++) {
        const dx = Math.abs(px - cx) - (hw - cornerR);
        const dy = Math.abs(py - cy) - (hh - cornerR);
        const ox = Math.max(dx, 0);
        const oy = Math.max(dy, 0);
        const outside = Math.hypot(ox, oy);
        const inside = Math.min(Math.max(dx, dy), 0);
        const sdf = outside + inside - cornerR; // < 0 inside keycap, > 0 outside
        const distIn = -sdf; // > 0 inside keycap

        // Encode continuous signed distance: B = 128 + distIn * (255 / 42)
        const bVal = Math.max(0, Math.min(255, Math.round(128 + distIn * (255 / 42))));
        const o = (py * MAP_W + px) * 4;
        // Keep the nearest keycap (largest distIn / B value) when bounding boxes overlap!
        if (bVal <= data[o + 2]) continue;

        const u = Math.max(-1, Math.min(1, (px - cx) / hw));
        const v = Math.max(-1, Math.min(1, (py - cy) / hh));

        // Signature Flux maglev vertical guide post on the left & right side walls of each keycap!
        let post = 0;
        if (distIn >= 1.5 && distIn <= 8.5) {
          const distFromSide = hw - Math.abs(px - cx);
          const absY = Math.abs(py - cy);
          if (distFromSide >= 2.0 && distFromSide <= 7.2 && absY <= 13.5) {
            const xEdge = Math.min(1, Math.min(distFromSide - 2.0, 7.2 - distFromSide) * 0.95);
            const yEdge = Math.min(1, (13.5 - absY) * 0.55);
            post = Math.round(Math.max(0, Math.min(1, xEdge * yEdge)) * 255);
          }
        }

        data[o] = Math.round((u * 0.5 + 0.5) * 255);
        data[o + 1] = Math.round((v * 0.5 + 0.5) * 255);
        data[o + 2] = bVal;
        data[o + 3] = post;
      }
    }
  }

  return data;
}

function compileShader(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader | null {
  const s = gl.createShader(type);
  if (!s) return null;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
    console.error("GlassKeyboardCanvas shader compile error:", gl.getShaderInfoLog(s));
    gl.deleteShader(s);
    return null;
  }
  return s;
}

export const GlassKeyboardCanvas: React.FC<GlassKeyboardCanvasProps> = ({
  keys,
  selectedKey,
  hoveredKey,
  previewTick,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const glRef = useRef<{
    gl: WebGL2RenderingContext;
    prog: WebGLProgram;
    texVideo: WebGLTexture;
    texOverlay: WebGLTexture;
    texGlass: WebGLTexture;
    hasVideo: boolean;
    hasOverlay: boolean;
    bgDim: number;
    topFade: number;
    selRect: [number, number, number, number];
    hovRect: [number, number, number, number];
  } | null>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  const [fallbackSrc, setFallbackSrc] = useState<string | null>(null);
  const [webglFailed, setWebglFailed] = useState(false);
  const activeBgNameRef = useRef<string | null>(null);

  // Update interactive selected / hovered rects for instant 60 FPS GPU rim illumination
  useEffect(() => {
    const st = glRef.current;
    if (!st) return;
    const toRect = (idx: number | null): [number, number, number, number] => {
      if (idx == null) return [0, 0, 0, 0];
      const k = keys.find((x) => x.index === idx);
      if (!k) return [0, 0, 0, 0];
      return [
        k.rect.x / DISPLAY_W,
        k.rect.y / DISPLAY_H,
        (k.rect.x + k.rect.w) / DISPLAY_W,
        (k.rect.y + k.rect.h) / DISPLAY_H,
      ];
    };
    st.selRect = toRect(selectedKey);
    st.hovRect = toRect(hoveredKey);
  }, [keys, selectedKey, hoveredKey]);

  // Initialize WebGL2 context + shaders + 60 FPS render loop
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const gl = canvas.getContext("webgl2", {
      alpha: false,
      antialias: true,
      powerPreference: "high-performance",
    });
    if (!gl) {
      setWebglFailed(true);
      return;
    }

    const vs = compileShader(gl, gl.VERTEX_SHADER, VERT_SRC);
    const fs = compileShader(gl, gl.FRAGMENT_SHADER, FRAG_SRC);
    if (!vs || !fs) {
      setWebglFailed(true);
      return;
    }
    const prog = gl.createProgram();
    if (!prog) {
      setWebglFailed(true);
      return;
    }
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      console.error("GlassKeyboardCanvas link error:", gl.getProgramInfoLog(prog));
      setWebglFailed(true);
      return;
    }

    const quad = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const aPos = gl.getAttribLocation(prog, "a_pos");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

    const makeTex = () => {
      const t = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([10, 14, 22, 255]));
      return t;
    };

    const texVideo = makeTex();
    const texOverlay = makeTex();
    const texGlass = makeTex();

    const state = {
      gl,
      prog,
      texVideo,
      texOverlay,
      texGlass,
      hasVideo: false,
      hasOverlay: false,
      bgDim: 0.0,
      topFade: 0.0,
      selRect: [0, 0, 0, 0] as [number, number, number, number],
      hovRect: [0, 0, 0, 0] as [number, number, number, number],
    };
    glRef.current = state;

    const uVideo = gl.getUniformLocation(prog, "u_video");
    const uOverlay = gl.getUniformLocation(prog, "u_overlay");
    const uGlass = gl.getUniformLocation(prog, "u_glass_map");
    const uHasVideo = gl.getUniformLocation(prog, "u_has_video");
    const uHasOverlay = gl.getUniformLocation(prog, "u_has_overlay");
    const uBgDim = gl.getUniformLocation(prog, "u_bg_dim");
    const uTopFade = gl.getUniformLocation(prog, "u_top_fade");
    const uTime = gl.getUniformLocation(prog, "u_time");
    const uSel = gl.getUniformLocation(prog, "u_sel_rect");
    const uHov = gl.getUniformLocation(prog, "u_hov_rect");

    let rafId = 0;
    const t0 = performance.now();
    const frame = () => {
      const vid = videoRef.current;
      if (vid && vid.readyState >= 2 && vid.videoWidth > 0) {
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, state.texVideo);
        try {
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, vid);
          state.hasVideo = true;
        } catch {
          // Ignore transient upload error while video switches source
        }
      }

      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.useProgram(prog);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, state.texVideo);
      gl.uniform1i(uVideo, 0);
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, state.texOverlay);
      gl.uniform1i(uOverlay, 1);
      gl.activeTexture(gl.TEXTURE2);
      gl.bindTexture(gl.TEXTURE_2D, state.texGlass);
      gl.uniform1i(uGlass, 2);

      gl.uniform1f(uHasVideo, state.hasVideo ? 1.0 : 0.0);
      gl.uniform1f(uHasOverlay, state.hasOverlay ? 1.0 : 0.0);
      gl.uniform1f(uBgDim, state.bgDim);
      gl.uniform1f(uTopFade, state.topFade);
      gl.uniform1f(uTime, (performance.now() - t0) / 1000.0);
      gl.uniform4fv(uSel, state.selRect);
      gl.uniform4fv(uHov, state.hovRect);

      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      rafId = window.requestAnimationFrame(frame);
    };
    rafId = window.requestAnimationFrame(frame);

    return () => {
      window.cancelAnimationFrame(rafId);
      glRef.current = null;
    };
  }, []);

  // Upload 3D acrylic normal/SDF map whenever keyboard geometry loads
  useEffect(() => {
    const st = glRef.current;
    if (!st || keys.length === 0) return;
    const pixels = buildGlassNormalMap(keys);
    const { gl, texGlass } = st;
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, texGlass);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, MAP_W, MAP_H, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  }, [keys]);

  // Poll crisp transparent overlay (key legends + top bar widgets + toasts) + active background media name
  useEffect(() => {
    let alive = true;
    const refreshOverlay = async () => {
      try {
        const res = await v2Client.renderPreviewFrame(2, 16.667, 960, 540, true);
        if (!alive) return;
        const st = glRef.current;
        if (st) {
          st.bgDim = typeof res.bg_dimming === "number" ? res.bg_dimming : 0;
          st.topFade = typeof res.top_fade === "number" ? res.top_fade : 0;
        }
        // Upload crisp transparent overlay PNG directly into WebGL2 texture 1
        if (res.png_base64) {
          const img = new Image();
          img.onload = () => {
            const cur = glRef.current;
            if (!alive || !cur) return;
            cur.gl.activeTexture(cur.gl.TEXTURE1);
            cur.gl.bindTexture(cur.gl.TEXTURE_2D, cur.texOverlay);
            cur.gl.texImage2D(cur.gl.TEXTURE_2D, 0, cur.gl.RGBA, cur.gl.RGBA, cur.gl.UNSIGNED_BYTE, img);
            cur.hasOverlay = true;
          };
          img.src = `data:image/png;base64,${res.png_base64}`;
        }

        // Resolve live 60 FPS background video (or shader fallback when no MP4 is active)
        const bgName = res.background_media_name || null;
        if (bgName && bgName !== activeBgNameRef.current) {
          activeBgNameRef.current = bgName;
          const lower = bgName.toLowerCase();
          if (lower.endsWith(".mp4") || lower.endsWith(".mov") || lower.endsWith(".webm")) {
            const url = await v2Client.getMediaUrl(bgName);
            if (alive) setVideoUrl(url);
          } else {
            const url = await v2Client.getMediaUrl(bgName);
            if (alive) {
              setVideoUrl(null);
              const bgImg = new Image();
              bgImg.crossOrigin = "anonymous";
              bgImg.onload = () => {
                const cur = glRef.current;
                if (!alive || !cur) return;
                cur.gl.activeTexture(cur.gl.TEXTURE0);
                cur.gl.bindTexture(cur.gl.TEXTURE_2D, cur.texVideo);
                cur.gl.texImage2D(cur.gl.TEXTURE_2D, 0, cur.gl.RGBA, cur.gl.RGBA, cur.gl.UNSIGNED_BYTE, bgImg);
                cur.hasVideo = true;
              };
              bgImg.src = url;
            }
          }
        } else if (!bgName) {
          activeBgNameRef.current = null;
          setVideoUrl(null);
          // Non-video background (e.g. Aurora GPU shader): grab full composite into u_video!
          const full = await v2Client.renderPreviewFrame(2, 16.667, 960, 540, false);
          if (alive && full.png_base64) {
            setFallbackSrc(`data:image/png;base64,${full.png_base64}`);
            const bgImg = new Image();
            bgImg.onload = () => {
              const cur = glRef.current;
              if (!alive || !cur) return;
              cur.gl.activeTexture(cur.gl.TEXTURE0);
              cur.gl.bindTexture(cur.gl.TEXTURE_2D, cur.texVideo);
              cur.gl.texImage2D(cur.gl.TEXTURE_2D, 0, cur.gl.RGBA, cur.gl.RGBA, cur.gl.UNSIGNED_BYTE, bgImg);
              cur.hasVideo = true;
              cur.hasOverlay = false; // full composite already includes overlay
            };
            bgImg.src = `data:image/png;base64,${full.png_base64}`;
          }
        }
      } catch {
        // ignore transient poll error
      }
    };
    void refreshOverlay();
    const id = window.setInterval(() => void refreshOverlay(), 1800);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [previewTick]);

  // Kick WKWebView video playback reliably whenever videoUrl updates
  useEffect(() => {
    const v = videoRef.current;
    if (!v || !videoUrl) return;
    v.src = videoUrl;
    v.load();
    void v.play().catch(() => {});
  }, [videoUrl]);

  if (webglFailed && fallbackSrc) {
    return <img src={fallbackSrc} alt="Keyglass Preview" className="kb-surface-preview-img" draggable={false} />;
  }

  return (
    <>
      <video
        ref={videoRef}
        autoPlay
        loop
        muted
        playsInline
        crossOrigin="anonymous"
        style={{ position: "fixed", width: 1, height: 1, opacity: 0.001, pointerEvents: "none" }}
      />
      <canvas
        ref={canvasRef}
        width={1920}
        height={1080}
        className="kb-surface-preview-img"
      />
    </>
  );
};
