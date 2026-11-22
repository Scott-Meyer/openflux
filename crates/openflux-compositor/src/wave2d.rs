//! Viscosity-damped 2D Wave Equation Engine (`480x270` quarter-res fixed 60 Hz PDE).
//!
//! Runs the exact water wave simulation tuned in the browser prototype:
//! - Quarter resolution (`480x270`, 1/4 of 1920x1080) in full `f32` precision (`h`, `vel`).
//! - Fixed `60 Hz` accumulator (capped at 4 steps per frame) so ring speed is frame-rate independent.
//! - Cosine bell drops injected on the first step of each frame (up to 8 drops/step):
//!   - **Ripple per key:** radius `46 px`, amplitude `0.9`
//!   - **Gentle Swell per key:** radius `140 px`, amplitude `0.5`
//! - Mild viscosity `h = mix(h, avg, 0.06)` eliminates grid-scale marbling noise inside rings,
//!   with damping `0.988` and `0.9995` gentle decay per step.
//! - Computes smooth `±1.5`-sim-texel central-difference gradients in full `f32` and packs:
//!   - `R, G`: `(gx, gy)` scaled to `[-0.18, +0.18]` for crisp background/video UV refraction.
//!   - `B`: full 8-bit `[0, 1]` sunlit facing `clamp(dot(grad, light_dir) * 22.0, 0.0, 1.0)` (zero banding!).
//!   - `A`: full 8-bit `[0, 1]` shadow trough `clamp(-dot(grad, light_dir) * 22.0, 0.0, 1.0)`.
//! - Automatically sleeps (`0.00 ms`) once energy decays below visibility threshold.

use openflux_bus::{DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX};
use openflux_modules::{WaveFieldParams, WaveImpulseCmd};
use std::f32::consts::PI;

pub const WAVE2D_W: usize = 480;
pub const WAVE2D_H: usize = 270;

const STEP_DT: f32 = 1.0 / 60.0;
const MAX_STEPS_PER_FRAME: usize = 4;
const MAX_DROPS_PER_STEP: usize = 8;
const VISCOSITY: f32 = 0.06;
const HEIGHT_DECAY: f32 = 0.9995;
const GRAD_ENCODE_MAX: f32 = 0.18;
// Top-left light direction in Y-down screen space (equivalent to (-0.7071, +0.7071) in Y-up WebGL).
const LIGHT_X: f32 = -0.70710678;
const LIGHT_Y: f32 = -0.70710678;
const FACING_GAIN: f32 = 22.0;

#[derive(Clone, Copy, Debug)]
struct PendingDrop {
    x_px: f32,
    y_px: f32,
    radius_px: f32,
    amp: f32,
}

pub struct WaveField2D {
    pub width: usize,
    pub height: usize,
    h: Vec<f32>,
    h_next: Vec<f32>,
    vel: Vec<f32>,
    vel_next: Vec<f32>,
    pending_drops: Vec<PendingDrop>,
    accum_sec: f32,
    active: bool,
    /// Packed `RGBA8` map (`R=gx, G=gy, B=facing_pos, A=facing_neg`).
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
        let mut rgba = vec![0u8; n * 4];
        Self::clear_rgba_neutral(&mut rgba);
        Self {
            width: WAVE2D_W,
            height: WAVE2D_H,
            h: vec![0.0; n],
            h_next: vec![0.0; n],
            vel: vec![0.0; n],
            vel_next: vec![0.0; n],
            pending_drops: Vec::with_capacity(MAX_DROPS_PER_STEP),
            accum_sec: 0.0,
            active: false,
            rgba,
            was_clear: true,
        }
    }

    fn clear_rgba_neutral(rgba: &mut [u8]) {
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 128;
            px[1] = 128;
            px[2] = 0;
            px[3] = 0;
        }
    }

    pub fn has_energy(&self) -> bool {
        self.active || !self.pending_drops.is_empty()
    }

    pub fn apply_impulse(&mut self, imp: &WaveImpulseCmd) {
        if self.pending_drops.len() >= MAX_DROPS_PER_STEP {
            self.pending_drops.remove(0);
        }
        self.pending_drops.push(PendingDrop {
            x_px: imp.x,
            y_px: imp.y,
            radius_px: imp.radius.clamp(12.0, 320.0),
            amp: imp.force.clamp(-2.5, 2.5),
        });
        self.active = true;
    }

    fn inject_pending_drops(&mut self) {
        if self.pending_drops.is_empty() {
            return;
        }
        let w = self.width;
        let h_dim = self.height;
        let sx = DISPLAY_WIDTH_PX as f32 / w as f32;
        let sy = DISPLAY_HEIGHT_PX as f32 / h_dim as f32;

        for drop in self.pending_drops.drain(..) {
            let rad = drop.radius_px;
            let inv_rad = 1.0 / rad;
            let x0 = (((drop.x_px - rad) / sx).floor() as isize).clamp(0, w as isize - 1) as usize;
            let x1 = (((drop.x_px + rad) / sx).ceil() as isize).clamp(0, w as isize - 1) as usize;
            let y0 = (((drop.y_px - rad) / sy).floor() as isize).clamp(0, h_dim as isize - 1) as usize;
            let y1 = (((drop.y_px + rad) / sy).ceil() as isize).clamp(0, h_dim as isize - 1) as usize;

            for y in y0..=y1 {
                let py = (y as f32 + 0.5) * sy;
                let dy = py - drop.y_px;
                let dy2 = dy * dy;
                let row = y * w;
                for x in x0..=x1 {
                    let px = (x as f32 + 0.5) * sx;
                    let dx = px - drop.x_px;
                    let d2 = dx * dx + dy2;
                    let r = d2.sqrt() * inv_rad;
                    if r < 1.0 {
                        self.h[row + x] += drop.amp * (0.5 + 0.5 * (r * PI).cos());
                    }
                }
            }
        }
    }

    fn step_once(&mut self, damp: f32) -> f32 {
        let w = self.width;
        let h_dim = self.height;
        let mut max_e = 0.0_f32;

        for y in 0..h_dim {
            let y_up = if y > 0 { y - 1 } else { 0 };
            let y_dn = if y + 1 < h_dim { y + 1 } else { h_dim - 1 };
            let row = y * w;
            let row_up = y_up * w;
            let row_dn = y_dn * w;

            for x in 0..w {
                let x_l = if x > 0 { x - 1 } else { 0 };
                let x_r = if x + 1 < w { x + 1 } else { w - 1 };
                let idx = row + x;

                let hc = self.h[idx];
                let avg = 0.25
                    * (self.h[row + x_l]
                        + self.h[row + x_r]
                        + self.h[row_up + x]
                        + self.h[row_dn + x]);
                let v = (self.vel[idx] + (avg - hc) * 2.0) * damp;
                let mut hn = (hc + v) * HEIGHT_DECAY;
                // Mild viscosity kills grid-scale marbling noise inside rings!
                hn = hn + (avg - hn) * VISCOSITY;

                self.h_next[idx] = hn;
                self.vel_next[idx] = v;

                let e = hn.abs().max(v.abs());
                if e > max_e {
                    max_e = e;
                }
            }
        }

        std::mem::swap(&mut self.h, &mut self.h_next);
        std::mem::swap(&mut self.vel, &mut self.vel_next);
        max_e
    }

    /// Steps the fixed-60Hz wave simulation and packs smooth `±1.5`-texel gradients + full-precision
    /// sunlit facing / shadow trough values into `self.rgba`.
    pub fn step_dt(&mut self, dt_sec: f32, params: &WaveFieldParams) {
        if !self.active && self.pending_drops.is_empty() {
            if !self.was_clear {
                Self::clear_rgba_neutral(&mut self.rgba);
                self.was_clear = true;
            }
            return;
        }

        let damp = params.damping.clamp(0.92, 0.996);
        self.accum_sec = (self.accum_sec + dt_sec.clamp(0.0, 0.1)).min(STEP_DT * MAX_STEPS_PER_FRAME as f32);

        let mut steps = 0usize;
        while self.accum_sec >= STEP_DT && steps < MAX_STEPS_PER_FRAME {
            self.accum_sec -= STEP_DT;
            steps += 1;
        }
        // Ensure a brand-new drop responds immediately on the very frame it arrives.
        if steps == 0 && !self.pending_drops.is_empty() {
            steps = 1;
        }
        if steps == 0 {
            return;
        }

        let mut max_e = 0.0_f32;
        for step_i in 0..steps {
            if step_i == 0 {
                self.inject_pending_drops();
            }
            max_e = self.step_once(damp);
        }

        if max_e < 0.00025 && self.pending_drops.is_empty() {
            self.h.fill(0.0);
            self.vel.fill(0.0);
            self.active = false;
            Self::clear_rgba_neutral(&mut self.rgba);
            self.was_clear = true;
            return;
        }

        self.was_clear = false;
        let w = self.width;
        let h_dim = self.height;
        let inv_grad_max = 0.5 / GRAD_ENCODE_MAX;

        // Exact ±1.5-texel bilinear central difference in full f32:
        // hl = 0.5*(h[x-1]+h[x-2]), hr = 0.5*(h[x+1]+h[x+2]) -> grad_x = (hr - hl) / 3.0
        for y in 0..h_dim {
            let ym1 = y.saturating_sub(1);
            let ym2 = y.saturating_sub(2);
            let yp1 = (y + 1).min(h_dim - 1);
            let yp2 = (y + 2).min(h_dim - 1);

            let row = y * w;
            let r_m1 = ym1 * w;
            let r_m2 = ym2 * w;
            let r_p1 = yp1 * w;
            let r_p2 = yp2 * w;

            for x in 0..w {
                let xm1 = x.saturating_sub(1);
                let xm2 = x.saturating_sub(2);
                let xp1 = (x + 1).min(w - 1);
                let xp2 = (x + 2).min(w - 1);

                let hl = 0.5 * (self.h[row + xm1] + self.h[row + xm2]);
                let hr = 0.5 * (self.h[row + xp1] + self.h[row + xp2]);
                let hu = 0.5 * (self.h[r_m1 + x] + self.h[r_m2 + x]);
                let hd = 0.5 * (self.h[r_p1 + x] + self.h[r_p2 + x]);

                let gx = (hr - hl) * (1.0 / 3.0);
                let gy = (hd - hu) * (1.0 / 3.0);

                // Compute sunlit facing & shadow trough in full f32 so all 256 levels of B & A
                // map cleanly to [0, 1] with ZERO banding/stair-stepping on fading rings!
                let facing = (gx * LIGHT_X + gy * LIGHT_Y) * FACING_GAIN;
                let pos_f = facing.clamp(0.0, 1.0);
                let neg_f = (-facing).clamp(0.0, 1.0);

                let o = (row + x) * 4;
                self.rgba[o] = ((0.5 + gx * inv_grad_max).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                self.rgba[o + 1] = ((0.5 + gy * inv_grad_max).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                self.rgba[o + 2] = (pos_f * 255.0 + 0.5) as u8;
                self.rgba[o + 3] = (neg_f * 255.0 + 0.5) as u8;
            }
        }
    }
}
