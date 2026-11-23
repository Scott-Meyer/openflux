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
const MAP_W = 960;
const MAP_H = 540;

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
uniform sampler2D u_glass_map;  // Precomputed 3D acrylic normal (RG), height (B), region (A)
uniform float u_has_video;
uniform float u_has_overlay;
uniform float u_bg_dim;
uniform float u_top_fade;
uniform float u_time;
uniform vec4 u_sel_rect;        // normalized (x0, y0, x1, y1) of selected key
uniform vec4 u_hov_rect;        // normalized (x0, y0, x1, y1) of hovered key

vec3 sample_screen(vec2 uv) {
  vec2 suv = clamp(uv, vec2(0.001), vec2(0.999));
  vec3 bg = vec3(0.03, 0.05, 0.09);
  if (u_has_video > 0.5) {
    bg = texture(u_video, suv).rgb;
    bg *= (1.0 - clamp(u_bg_dim, 0.0, 0.85));
  }
  // Full-bleed uniform GPU top-bar dimming matching the RK3566 compositor
  if (u_top_fade > 0.001 && suv.y < 0.27) {
    float fade_t = 1.0 - smoothstep(0.24, 0.27, suv.y);
    bg *= (1.0 - u_top_fade * fade_t);
  }
  if (u_has_overlay > 0.5) {
    vec4 ov = texture(u_overlay, suv);
    bg = mix(bg, ov.rgb, ov.a);
  }
  return bg;
}

float in_rect(vec2 uv, vec4 r) {
  if (r.z <= r.x || r.w <= r.y) return 0.0;
  return step(r.x, uv.x) * step(uv.x, r.z) * step(r.y, uv.y) * step(uv.y, r.w);
}

void main() {
  vec4 gm = texture(u_glass_map, v_uv);
  vec2 nxy = gm.rg * 2.0 - 1.0;
  float h = gm.b;
  float reg = gm.a; // ~0.0=trench, ~0.4=bevel wall, ~0.75=keycap face, ~1.0=top clear strip

  // Construct 3D unit normal for acrylic surface
  float nz = sqrt(max(0.08, 1.0 - dot(nxy, nxy)));
  vec3 N = normalize(vec3(nxy, nz));
  vec3 V = vec3(0.0, 0.14, 0.99);

  // Two soft studio overhead lights (main key softbox top-left + subtle cool rim light top-right)
  vec3 L1 = normalize(vec3(-0.28, -0.52, 0.81));
  vec3 L2 = normalize(vec3(0.42, -0.35, 0.84));
  vec3 H1 = normalize(L1 + V);
  vec3 H2 = normalize(L2 + V);

  float fresnel = pow(1.0 - max(dot(N, V), 0.0), 3.2);
  float spec1 = pow(max(dot(N, H1), 0.0), 56.0);
  float spec2 = pow(max(dot(N, H2), 0.0), 28.0);

  vec3 col;

  if (reg > 0.90) {
    // 1. Clear Acrylic Top Display Strip
    vec3 scr = sample_screen(v_uv);
    // Subtle overhead acrylic glass reflection across the top display strip
    float sheen = smoothstep(0.18, 0.0, v_uv.y) * 0.075;
    // Crisp frosted acrylic horizontal ridge separating Top Strip from Keycap Deck
    float ridge = smoothstep(0.0045, 0.0, abs(v_uv.y - 0.244));
    col = scr + vec3(0.82, 0.89, 1.0) * (sheen + ridge * 0.22);
  } else if (reg < 0.18) {
    // 2. Recessed Matte-Black Optical Key Trench (ambient occlusion around keycaps)
    vec3 scr = sample_screen(v_uv);
    col = scr * 0.28 + vec3(0.015, 0.018, 0.026);
  } else if (reg < 0.58) {
    // 3. Steep Sloped 3D Acrylic Keycap Bevel Wall (IOR = 1.51 Snell refraction + prism dispersion!)
    // Strong inward optical bending through 9mm clear acrylic side walls
    vec2 refr = nxy * 0.018;
    float r_ch = sample_screen(v_uv + refr * 1.12).r;
    float g_ch = sample_screen(v_uv + refr * 1.00).g;
    float b_ch = sample_screen(v_uv + refr * 0.88).b;
    vec3 refr_col = vec3(r_ch, g_ch, b_ch);

    // Internal acrylic wall brightening + Fresnel glass rim + overhead softbox specular catch
    float edge_rim = smoothstep(0.15, 0.85, h);
    vec3 glass_tint = vec3(0.84, 0.91, 1.0);
    col = refr_col * (0.86 + 0.18 * edge_rim);
    col += glass_tint * (fresnel * 0.34 + spec1 * 0.55 + spec2 * 0.20);
    // Subtle dark base shadow at the bottom of the bevel where acrylic meets the keyframe
    col *= mix(0.68, 1.04, smoothstep(0.0, 0.45, h));
  } else {
    // 4. Sculpted Crystal-Clear Keycap Top Face (gentle spherical dish magnification!)
    vec2 lens_uv = v_uv + nxy * 0.0045;
    vec3 scr = sample_screen(lens_uv);
    // Crisp, clear optical transmission with very subtle studio glass highlight
    float dish_sheen = spec1 * 0.16 + spec2 * 0.07 + fresnel * 0.08;
    // Slight bright micro-bevel ring right where the top face meets the sloped wall
    float inner_lip = smoothstep(0.88, 0.82, h) * 0.15;
    col = scr * 1.04 + vec3(0.86, 0.92, 1.0) * (dish_sheen + inner_lip);
  }

  // Interactive 3D internal acrylic edge glow on Hovered / Selected key
  float is_hov = in_rect(v_uv, u_hov_rect);
  float is_sel = in_rect(v_uv, u_sel_rect);
  if (is_hov > 0.5 && reg >= 0.18 && reg < 0.90) {
    float wall_boost = (reg < 0.58) ? 0.34 : 0.07;
    col += vec3(0.62, 0.52, 1.0) * wall_boost;
  }
  if (is_sel > 0.5 && reg >= 0.18 && reg < 0.90) {
    float wall_boost = (reg < 0.58) ? 0.55 : 0.12;
    col += vec3(0.45, 0.72, 1.0) * wall_boost;
  }

  // Subtle physical vignette toward outer bezel edges
  vec2 edge = min(v_uv, 1.0 - v_uv);
  float vig = smoothstep(0.0, 0.015, edge.x) * smoothstep(0.0, 0.02, edge.y);
  col *= mix(0.76, 1.0, vig);

  out_color = vec4(clamp(col, 0.0, 1.0), 1.0);
}
`;

/** Precomputes a 960x540 3D normal/bevel/region texture for all 85 sculpted Flux acrylic keycaps. */
function buildGlassNormalMap(keys: SurfaceKeyGeom[]): Uint8Array {
  const data = new Uint8Array(MAP_W * MAP_H * 4);
  const sx = MAP_W / DISPLAY_W;
  const sy = MAP_H / DISPLAY_H;

  // Initialize everything to trench (nx=0, ny=0, h=0, reg=0)
  for (let i = 0; i < MAP_W * MAP_H; i++) {
    const o = i * 4;
    data[o] = 128;
    data[o + 1] = 128;
    data[o + 2] = 0;
    data[o + 3] = 0;
  }

  // Top Clear Display Strip (y = 0..264 in 1080p)
  const topH = Math.round(264 * sy);
  for (let y = 0; y < topH; y++) {
    for (let x = 0; x < MAP_W; x++) {
      const o = (y * MAP_W + x) * 4;
      data[o] = 128;
      data[o + 1] = 128;
      data[o + 2] = 255;
      data[o + 3] = 255; // reg = 1.0 (top strip)
    }
  }

  // Rasterize each of the 85 sculpted acrylic keycaps with realistic sloped bevels + subtle dish
  for (const k of keys) {
    const x0 = Math.max(0, Math.floor((k.rect.x + 2) * sx));
    const y0 = Math.max(topH, Math.floor((k.rect.y + 2) * sy));
    const x1 = Math.min(MAP_W - 1, Math.ceil((k.rect.x + k.rect.w - 2) * sx));
    const y1 = Math.min(MAP_H - 1, Math.ceil((k.rect.y + k.rect.h - 2) * sy));
    const cx = (x0 + x1) * 0.5;
    const cy = (y0 + y1) * 0.5;
    const hw = Math.max(4, (x1 - x0) * 0.5);
    const hh = Math.max(4, (y1 - y0) * 0.5);
    const cornerR = 4.5;
    const bevelW = 6.2; // crisp sloped acrylic wall width in map pixels

    for (let py = y0; py <= y1; py++) {
      for (let px = x0; px <= x1; px++) {
        const dx = Math.abs(px - cx) - (hw - cornerR);
        const dy = Math.abs(py - cy) - (hh - cornerR);
        const ox = Math.max(dx, 0);
        const oy = Math.max(dy, 0);
        const outside = Math.hypot(ox, oy);
        const inside = Math.min(Math.max(dx, dy), 0);
        const sdf = outside + inside - cornerR; // < 0 inside keycap

        if (sdf > 0) continue; // recessed dark trench

        const distIn = -sdf;
        const o = (py * MAP_W + px) * 4;

        // Outward normal direction from SDF
        let gx = 0;
        let gy = 0;
        if (outside > 0.001) {
          gx = (ox / outside) * (px < cx ? -1 : 1);
          gy = (oy / outside) * (py < cy ? -1 : 1);
        } else if (dx > dy) {
          gx = px < cx ? -1 : 1;
        } else {
          gy = py < cy ? -1 : 1;
        }

        if (distIn < bevelW) {
          // Steep sloped 3D acrylic side wall!
          const t = distIn / bevelW; // 0 at base -> 1 at top lip
          const slope = 0.82 * (1.0 - t * 0.35);
          const nx = gx * slope;
          const ny = gy * slope;
          data[o] = Math.round((nx * 0.5 + 0.5) * 255);
          data[o + 1] = Math.round((ny * 0.5 + 0.5) * 255);
          data[o + 2] = Math.round(t * 210);
          data[o + 3] = 102; // reg ~ 0.40 (bevel wall)
        } else {
          // Slightly concave cylindrical-dish clear crystal top face!
          const u = (px - cx) / hw;
          const v = (py - cy) / hh;
          const nx = -u * 0.16;
          const ny = -v * 0.14;
          const lipFactor = Math.min(1.0, (distIn - bevelW) / 3.0);
          data[o] = Math.round((nx * 0.5 + 0.5) * 255);
          data[o + 1] = Math.round((ny * 0.5 + 0.5) * 255);
          data[o + 2] = Math.round(212 + lipFactor * 43);
          data[o + 3] = 192; // reg ~ 0.75 (clear top face)
        }
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
    return <img src={fallbackSrc} alt="KeyGlass Preview" className="kb-surface-preview-img" draggable={false} />;
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
        width={1280}
        height={720}
        className="kb-surface-preview-img"
      />
    </>
  );
};
