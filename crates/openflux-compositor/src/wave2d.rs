//! Analytical single-crest 3D outgoing water ripple engine (`384x216` normal/height map).
//!
//! Each key press (or shared `fx/surface/impulse` from Mochi the Idle Pet or CLI) spawns ONE
//! clean outgoing 3D water ring—matching the reach and calm clarity of v1's single outgoing
//! ripple while dissipating slightly faster and bending whatever video/background is playing
//! underneath with crisp 3D normals and specular highlights (zero choppy trailing wakes).

use openflux_bus::{DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX};
use openflux_modules::{WaveFieldParams, WaveImpulseCmd};

pub const WAVE2D_W: usize = 384;
pub const WAVE2D_H: usize = 216;
const MAX_ACTIVE_RINGS: usize = 48;

#[derive(Clone, Copy, Debug)]
struct RippleRing {
    cx: f32,
    cy: f32,
    base_r: f32,
    max_travel: f32,
    duration_sec: f32,
    age_sec: f32,
    band_width: f32,
    force: f32,
}

pub struct WaveField2D {
    pub width: usize,
    pub height: usize,
    rings: Vec<RippleRing>,
    /// Packed `RGBA8` normal/height map (`R = 0.5 + gx`, `G = 0.5 + gy`, `B = 0.5 + h`, `A = 255`).
    pub rgba: Vec<u8>,
    was_clear: bool,
}

impl Default for WaveField2D {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveField2D {
    pub fn new() -> Self {
        let n = WAVE2D_W * WAVE2D_H;
        let mut rgba = vec![128u8; n * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Self {
            width: WAVE2D_W,
            height: WAVE2D_H,
            rings: Vec::with_capacity(MAX_ACTIVE_RINGS),
            rgba,
            was_clear: true,
        }
    }

    pub fn has_energy(&self) -> bool {
        !self.rings.is_empty()
    }

    pub fn apply_impulse(&mut self, imp: &WaveImpulseCmd) {
        if self.rings.len() >= MAX_ACTIVE_RINGS {
            self.rings.remove(0);
        }
        // Single clean ring matching v1's ~2.4 key-width reach (~245 px), dissipating cleanly!
        let key_unit = 105.0_f32;
        let radius_scale = (imp.radius / 52.0).clamp(0.6, 1.8);
        self.rings.push(RippleRing {
            cx: imp.x,
            cy: imp.y,
            base_r: key_unit * 0.32,
            max_travel: key_unit * 1.95 * radius_scale,
            duration_sec: 0.90,
            age_sec: 0.0,
            band_width: 10.5,
            force: imp.force.clamp(-2.0, 2.0),
        });
    }

    /// Steps all active single-crest rings by `dt_sec` and renders their exact analytical 3D
    /// surface gradients `(gx, gy, h)` into `self.rgba` (neutral 128 everywhere outside rings).
    pub fn step_dt(&mut self, dt_sec: f32, params: &WaveFieldParams) {
        // `speed` scales ring propagation rate (`0.5` = baseline ~1.05s, `0.75` = snappy ~0.75s).
        // `damping` modulates dissipation lifetime (`0.935` = crisp fast fade, `0.98` = longer glide).
        let speed_mul = (params.speed / 0.52).clamp(0.35, 2.2);
        let damp_life = ((params.damping - 0.86) / 0.09).clamp(0.45, 1.65);

        for r in &mut self.rings {
            r.age_sec += dt_sec * speed_mul;
            r.duration_sec = 0.95 * damp_life;
        }
        self.rings.retain(|r| r.age_sec < r.duration_sec);

        if self.rings.is_empty() {
            if !self.was_clear {
                for px in self.rgba.chunks_exact_mut(4) {
                    px[0] = 128;
                    px[1] = 128;
                    px[2] = 128;
                    px[3] = 255;
                }
                self.was_clear = true;
            }
            return;
        }
        self.was_clear = false;

        // Reset field to flat glass neutral (0.5, 0.5, 0.5, 1.0)
        for px in self.rgba.chunks_exact_mut(4) {
            px[0] = 128;
            px[1] = 128;
            px[2] = 128;
            px[3] = 255;
        }

        let w = self.width;
        let h = self.height;
        let sx = DISPLAY_WIDTH_PX as f32 / w as f32;
        let sy = DISPLAY_HEIGHT_PX as f32 / h as f32;

        // Accumulate float gradients only inside each ring's tight bounding box.
        let mut gx_acc = vec![0.0_f32; w * h];
        let mut gy_acc = vec![0.0_f32; w * h];
        let mut h_acc = vec![0.0_f32; w * h];
        let mut touched_min_y = h;
        let mut touched_max_y = 0usize;
        let mut touched_min_x = w;
        let mut touched_max_x = 0usize;

        for ring in &self.rings {
            let pt = (ring.age_sec / ring.duration_sec).clamp(0.0, 1.0);
            // Smooth v1-style ease-out expansion: brisk launch, graceful glide, fast clean fade.
            let ease = 1.0 - (1.0 - pt) * (1.0 - pt);
            let cur_r = ring.base_r + ease * ring.max_travel;
            // Faster dissipation curve: ramps in smoothly in first 4%, fades out cleanly as (1-pt)^1.9.
            let fade_in = (pt / 0.05).min(1.0);
            let env = fade_in * (1.0 - pt).powf(1.9) * ring.force;
            if env.abs() < 0.004 {
                continue;
            }

            let bw = ring.band_width * (0.9 + 0.25 * pt);
            let outer_px = cur_r + bw * 2.4;
            let inner_px = (cur_r - bw * 2.4).max(0.0);
            let outer2 = outer_px * outer_px;
            let inner2 = inner_px * inner_px;
            let inv_bw = 1.0 / bw;

            let x0 = (((ring.cx - outer_px) / sx).floor() as isize).clamp(0, w as isize - 1) as usize;
            let x1 = (((ring.cx + outer_px) / sx).ceil() as isize).clamp(0, w as isize - 1) as usize;
            let y0 = (((ring.cy - outer_px) / sy).floor() as isize).clamp(0, h as isize - 1) as usize;
            let y1 = (((ring.cy + outer_px) / sy).ceil() as isize).clamp(0, h as isize - 1) as usize;

            touched_min_x = touched_min_x.min(x0);
            touched_max_x = touched_max_x.max(x1);
            touched_min_y = touched_min_y.min(y0);
            touched_max_y = touched_max_y.max(y1);

            for py in y0..=y1 {
                let dy = (py as f32 + 0.5) * sy - ring.cy;
                let dy2 = dy * dy;
                let row = py * w;
                for px in x0..=x1 {
                    let dx = (px as f32 + 0.5) * sx - ring.cx;
                    let d2 = dx * dx + dy2;
                    if d2 > outer2 || d2 < inner2 || d2 < 1.0 {
                        continue;
                    }
                    let dist = d2.sqrt();
                    let u = (dist - cur_r) * inv_bw;
                    let g = (-u * u).exp();
                    // Single unified outward wave-front lens (one peak in both displacement and crest,
                    // eliminating the double-ring split caused by opposing inner/outer slopes).
                    let height = g * env;
                    let displace = g * env * 0.95;
                    let inv_d = 1.0 / dist;
                    let i = row + px;
                    gx_acc[i] += displace * dx * inv_d;
                    gy_acc[i] += displace * dy * inv_d;
                    h_acc[i] += height;
                }
            }
        }

        if touched_min_y <= touched_max_y && touched_min_x <= touched_max_x {
            for py in touched_min_y..=touched_max_y {
                let row = py * w;
                for px in touched_min_x..=touched_max_x {
                    let i = row + px;
                    let p = i * 4;
                    self.rgba[p] = ((0.5 + gx_acc[i] * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                    self.rgba[p + 1] = ((0.5 + gy_acc[i] * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                    self.rgba[p + 2] = ((0.5 + h_acc[i] * 0.35).clamp(0.0, 1.0) * 255.0) as u8;
                    self.rgba[p + 3] = 255;
                }
            }
        }
    }

    /// Convenience wrapper assuming 60fps (`dt = 1/60s`).
    pub fn step(&mut self, params: &WaveFieldParams) {
        self.step_dt(1.0 / 60.0, params);
    }
}
