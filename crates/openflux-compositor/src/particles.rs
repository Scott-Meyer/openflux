//! Precomputed Key Layout SDF grid and key-bouncing / key-refracting particle simulation.
//!
//! Because the Flux keyboard has transparent lens-like keycaps separated by 2mm trenches,
//! particles emitted from pressed keys either ricochet along the 2mm trenches off keycap walls
//! or pass underneath a clear keycap with optical magnification and refraction.

use openflux_bus::{KeyboardGeometry, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX};
use openflux_modules::ParticleSpawnCmd;

/// Quarter-resolution SDF grid (480x270) gives 4px spatial resolution with sub-millisecond build
/// and O(1) bilinear lookup per particle step.
pub const SDF_GRID_W: usize = 480;
pub const SDF_GRID_H: usize = 270;

#[derive(Clone, Copy, Debug)]
pub struct SdfCell {
    pub dist_px: f32,
    pub nx: f32,
    pub ny: f32,
    pub key_index: u16,
}

pub struct KeySdfGrid {
    pub width: usize,
    pub height: usize,
    cells: Vec<SdfCell>,
}

impl KeySdfGrid {
    pub fn build(geom: &KeyboardGeometry) -> Self {
        let mut cells = Vec::with_capacity(SDF_GRID_W * SDF_GRID_H);
        let scale_x = DISPLAY_WIDTH_PX as f32 / SDF_GRID_W as f32;
        let scale_y = DISPLAY_HEIGHT_PX as f32 / SDF_GRID_H as f32;

        for gy in 0..SDF_GRID_H {
            let py = (gy as f32 + 0.5) * scale_y;
            for gx in 0..SDF_GRID_W {
                let px = (gx as f32 + 0.5) * scale_x;
                let s = geom.sample_key_sdf(px, py);
                cells.push(SdfCell {
                    dist_px: s.signed_distance_px,
                    nx: s.normal_x,
                    ny: s.normal_y,
                    key_index: s.nearest_key_index,
                });
            }
        }
        Self {
            width: SDF_GRID_W,
            height: SDF_GRID_H,
            cells,
        }
    }

    pub fn sample(&self, px: f32, py: f32) -> SdfCell {
        let gx = ((px / DISPLAY_WIDTH_PX as f32) * self.width as f32)
            .clamp(0.0, (self.width - 1) as f32) as usize;
        let gy = ((py / DISPLAY_HEIGHT_PX as f32) * self.height as f32)
            .clamp(0.0, (self.height - 1) as f32) as usize;
        self.cells[gy * self.width + gx]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LiveParticle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub radius_px: f32,
    pub age_ms: f32,
    pub life_ms: f32,
    pub color: [f32; 4],
    pub bounce_chance: f32,
    pub lens_refract: f32,
    pub drag: f32,
    pub inside_key: bool,
    pub current_scale: f32,
    pub rng_state: u32,
}

impl LiveParticle {
    fn next_rand01(&mut self) -> f32 {
        // Fast xorshift32 for per-particle bounce/refract decisions.
        let mut x = self.rng_state;
        if x == 0 {
            x = 0x9E37_79B9;
        }
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        (x as f32) / (u32::MAX as f32)
    }
}

const MAX_LIVE_PARTICLES: usize = 1024;

pub struct ParticleSystem {
    pub particles: Vec<LiveParticle>,
    spawn_counter: u32,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self {
            particles: Vec::with_capacity(256),
            spawn_counter: 1,
        }
    }
}

impl ParticleSystem {
    pub fn spawn(&mut self, cmd: &ParticleSpawnCmd, sdf: &KeySdfGrid) {
        if self.particles.len() >= MAX_LIVE_PARTICLES {
            self.particles.remove(0);
        }
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let seed = self
            .spawn_counter
            .wrapping_mul(0x85EB_CA6B)
            ^ (cmd.x.to_bits().rotate_left(7))
            ^ (cmd.y.to_bits().rotate_right(11));
        let start_inside = sdf.sample(cmd.x, cmd.y).dist_px < 0.0;
        self.particles.push(LiveParticle {
            x: cmd.x,
            y: cmd.y,
            vx: cmd.vx,
            vy: cmd.vy,
            radius_px: cmd.radius_px.max(1.5),
            age_ms: 0.0,
            life_ms: cmd.life_ms.max(50.0),
            color: cmd.color,
            bounce_chance: cmd.bounce_chance.clamp(0.0, 1.0),
            lens_refract: cmd.lens_refract.clamp(1.0, 2.5),
            drag: cmd.drag.clamp(0.8, 1.0),
            inside_key: start_inside,
            current_scale: if start_inside { cmd.lens_refract } else { 1.0 },
            rng_state: seed,
        });
    }

    pub fn step(&mut self, dt_sec: f32, sdf: &KeySdfGrid, geom: &KeyboardGeometry) {
        let dt_ms = dt_sec * 1000.0;
        let substeps = 2;
        let sub_dt = dt_sec / substeps as f32;
        let drag_step = (sub_dt * 60.0).max(0.0);

        for p in &mut self.particles {
            p.age_ms += dt_ms;
            if p.age_ms >= p.life_ms {
                continue;
            }
            let damping = p.drag.powf(drag_step);
            for _ in 0..substeps {
                p.vx *= damping;
                p.vy *= damping;
                let nx_pos = p.x + p.vx * sub_dt;
                let ny_pos = p.y + p.vy * sub_dt;

                let cell = sdf.sample(nx_pos, ny_pos);
                let now_inside = cell.dist_px < 0.0;

                if !p.inside_key && now_inside {
                    // Approaching a keycap wall from a 2mm trench!
                    let dot = p.vx * cell.nx + p.vy * cell.ny;
                    if dot < 0.0 && p.next_rand01() < p.bounce_chance {
                        // Bounce off the keycap wall into the 2mm trench.
                        let restitution = 0.86;
                        p.vx = (p.vx - 2.0 * dot * cell.nx) * restitution;
                        p.vy = (p.vy - 2.0 * dot * cell.ny) * restitution;
                        p.x += cell.nx * (cell.dist_px.abs() + 1.5);
                        p.y += cell.ny * (cell.dist_px.abs() + 1.5);
                        continue;
                    } else {
                        // Enter underneath the clear lens keycap with optical refraction bend.
                        if let Some(key) = geom.key(cell.key_index) {
                            let to_cx = key.center_x - nx_pos;
                            let to_cy = key.center_y - ny_pos;
                            let d = (to_cx * to_cx + to_cy * to_cy).sqrt().max(1.0);
                            p.vx += (to_cx / d) * 35.0;
                            p.vy += (to_cy / d) * 35.0;
                        }
                        p.inside_key = true;
                    }
                } else if p.inside_key && !now_inside {
                    // Exiting out from under the clear keycap.
                    p.inside_key = false;
                }

                p.x = nx_pos;
                p.y = ny_pos;

                // Soft screen boundary bounce.
                if p.x < 4.0 {
                    p.x = 4.0;
                    p.vx = p.vx.abs() * 0.7;
                } else if p.x > DISPLAY_WIDTH_PX as f32 - 4.0 {
                    p.x = DISPLAY_WIDTH_PX as f32 - 4.0;
                    p.vx = -p.vx.abs() * 0.7;
                }
                if p.y < 4.0 {
                    p.y = 4.0;
                    p.vy = p.vy.abs() * 0.7;
                } else if p.y > DISPLAY_HEIGHT_PX as f32 - 4.0 {
                    p.y = DISPLAY_HEIGHT_PX as f32 - 4.0;
                    p.vy = -p.vy.abs() * 0.7;
                }
            }

            // Smooth lens magnification when under a keycap.
            let target_scale = if p.inside_key { p.lens_refract } else { 1.0 };
            p.current_scale += (target_scale - p.current_scale) * (dt_sec * 18.0).min(1.0);
        }

        self.particles.retain(|p| p.age_ms < p.life_ms);
    }
}
