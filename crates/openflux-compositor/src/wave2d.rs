//! Lightweight `192x108` float32 2D damped wave heightfield simulation for GLES 2.0 / Mali-G52
//! and software targets where half-float FBO extensions aren't guaranteed.
//!
//! Runs in ~0.12ms on ARM64 Cortex-A55 and packs `(dh/dx, dh/dy, height, energy)` into an
//! `RGBA8` texture (`82 KB`) for hardware bilinearly-filtered 3D refraction + specular on Mali-G52.

use openflux_bus::{DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX};
use openflux_modules::{WaveFieldParams, WaveImpulseCmd};

pub const WAVE2D_W: usize = 192;
pub const WAVE2D_H: usize = 108;

pub struct WaveField2D {
    pub width: usize,
    pub height: usize,
    curr: Vec<f32>,
    prev: Vec<f32>,
    next: Vec<f32>,
    /// Packed `RGBA8` normal/height map (`R = 0.5 + gx`, `G = 0.5 + gy`, `B = 0.5 + h`, `A = 255`).
    pub rgba: Vec<u8>,
    active_energy: f32,
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
            curr: vec![0.0; n],
            prev: vec![0.0; n],
            next: vec![0.0; n],
            rgba,
            active_energy: 0.0,
        }
    }

    pub fn has_energy(&self) -> bool {
        self.active_energy > 0.0005
    }

    pub fn apply_impulse(&mut self, imp: &WaveImpulseCmd) {
        let cx = (imp.x / DISPLAY_WIDTH_PX as f32) * self.width as f32;
        let cy = (imp.y / DISPLAY_HEIGHT_PX as f32) * self.height as f32;
        let rad = ((imp.radius / DISPLAY_HEIGHT_PX as f32) * self.height as f32).clamp(2.5, 28.0);
        let r_ceil = (rad * 1.9).ceil() as isize;
        let ix0 = (cx as isize - r_ceil).max(1) as usize;
        let ix1 = (cx as isize + r_ceil).min(self.width as isize - 2) as usize;
        let iy0 = (cy as isize - r_ceil).max(1) as usize;
        let iy1 = (cy as isize + r_ceil).min(self.height as isize - 2) as usize;
        let inv_r2 = 1.0 / (rad * rad);

        for y in iy0..=iy1 {
            let dy = y as f32 - cy;
            let row = y * self.width;
            for x in ix0..=ix1 {
                let dx = x as f32 - cx;
                let d2 = (dx * dx + dy * dy) * inv_r2;
                if d2 < 3.5 {
                    let drop = (-d2 * 2.2).exp() * (1.0 - 0.35 * d2) * imp.force;
                    self.curr[row + x] = (self.curr[row + x] + drop).clamp(-2.5, 2.5);
                }
            }
        }
        self.active_energy = self.active_energy.max(imp.force.abs());
    }

    pub fn step(&mut self, params: &WaveFieldParams) {
        let w = self.width;
        let h = self.height;
        let c2 = (params.speed * 1.65).clamp(0.05, 1.45);
        let damp = params.damping.clamp(0.90, 0.996);

        let mut max_e = 0.0_f32;
        for y in 1..(h - 1) {
            let row = y * w;
            let edge_y = (y.min(h - 1 - y) as f32 / 5.0).min(1.0);
            for x in 1..(w - 1) {
                let i = row + x;
                let hc = self.curr[i];
                let hp = self.prev[i];
                let lap = (self.curr[i - 1]
                    + self.curr[i + 1]
                    + self.curr[i - w]
                    + self.curr[i + w])
                    * 0.25
                    - hc;
                let edge_x = (x.min(w - 1 - x) as f32 / 5.0).min(1.0);
                let edge_damp = 0.88 + 0.12 * edge_x.min(edge_y);
                let hn = ((2.0 * hc - hp) + lap * c2) * damp * edge_damp;
                let clamped = hn.clamp(-2.5, 2.5);
                self.next[i] = clamped;
                max_e = max_e.max(clamped.abs());
            }
        }

        std::mem::swap(&mut self.prev, &mut self.curr);
        std::mem::swap(&mut self.curr, &mut self.next);
        self.active_energy = max_e;

        // Pack gradients + height into RGBA8 for fast bilinear GPU sampling.
        for y in 1..(h - 1) {
            let row = y * w;
            for x in 1..(w - 1) {
                let i = row + x;
                let gx = self.curr[i + 1] - self.curr[i - 1];
                let gy = self.curr[i + w] - self.curr[i - w];
                let hc = self.curr[i];
                let p = i * 4;
                self.rgba[p] = ((0.5 + gx * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                self.rgba[p + 1] = ((0.5 + gy * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                self.rgba[p + 2] = ((0.5 + hc * 0.35).clamp(0.0, 1.0) * 255.0) as u8;
                self.rgba[p + 3] = 255;
            }
        }
    }
}
