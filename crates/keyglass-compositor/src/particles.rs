//! Precomputed key layout SDF and keypress particle simulation.
//!
//! Sparks can enter clear keycaps freely, appear magnified inside, and ricochet off inner side
//! and bottom walls. Gravity brings them to a crowned floor where they pile up and spill out.

use keyglass_bus::{KeyboardGeometry, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX};
use keyglass_modules::ParticleSpawnCmd;

/// Quarter-resolution SDF grid (480x270) gives 4px spatial resolution with sub-millisecond build
/// and O(1) lookup per particle step.
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
    bottom_row: u8,
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
            bottom_row: geom.keys.iter().map(|key| key.row).max().unwrap_or(0),
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
    has_landed: bool,
}

/// A plain, alpha-fading circle in display pixels. The position and radius form one vertex
/// attribute and the color forms the other when the keyboard draws these as point sprites.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ParticleDot {
    pub center_px: [f32; 2],
    pub radius_px: f32,
    pub color: [f32; 4],
}

impl LiveParticle {
    pub fn dot(&self) -> ParticleDot {
        let fade = ((self.life_ms - self.age_ms) / self.life_ms.min(1_250.0)).clamp(0.0, 1.0);
        let mut color = self.color;
        color[3] *= fade;
        ParticleDot {
            center_px: [self.x, self.y],
            radius_px: self.radius_px * self.current_scale,
            color,
        }
    }

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
        (x >> 8) as f32 / 16_777_216.0
    }
}

// Bound the collision and drawing workload during sustained typing.
const MAX_LIVE_PARTICLES: usize = 384;
const WIDTH: f32 = DISPLAY_WIDTH_PX as f32;
const HEIGHT: f32 = DISPLAY_HEIGHT_PX as f32;
const GRAVITY: f32 = 1_250.0; // Display pixels per second squared.
const REST_SPEED: f32 = 45.0;

pub struct ParticleSystem {
    pub particles: Vec<LiveParticle>,
    spawn_counter: u32,
    contacts: ContactGrid,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self {
            particles: Vec::with_capacity(256),
            spawn_counter: 1,
            contacts: ContactGrid::default(),
        }
    }
}

impl ParticleSystem {
    pub fn spawn(&mut self, cmd: &ParticleSpawnCmd, sdf: &KeySdfGrid) {
        if self.particles.len() >= MAX_LIVE_PARTICLES {
            self.particles.remove(0);
        }
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let seed = self.spawn_counter.wrapping_mul(0x85EB_CA6B)
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
            has_landed: false,
        });
    }

    pub fn step(&mut self, dt_sec: f32, sdf: &KeySdfGrid, geom: &KeyboardGeometry) {
        if !dt_sec.is_finite() || dt_sec <= 0.0 {
            return;
        }
        for p in &mut self.particles {
            p.age_ms += dt_sec * 1000.0;
        }
        self.particles.retain(|p| p.age_ms < p.life_ms);
        if self.particles.is_empty() {
            return;
        }

        // Resolve contacts during flight, not after the whole frame has gone by. Limit each
        // step to roughly a radius of travel so two small, fast sparks don't swap sides.
        // Bound catch-up work after a stall, but let lifetimes track the full elapsed time.
        let sim_dt = dt_sec.min(0.1);
        let mut max_speed: f32 = 0.0;
        let mut travel_px: f32 = 6.0;
        let mut cell_size: f32 = 16.0;
        for p in &self.particles {
            max_speed = max_speed.max(p.vx.hypot(p.vy));
            travel_px = travel_px.min(p.radius_px);
            cell_size = cell_size.max(2.0 * p.radius_px);
        }
        let max_travel = (max_speed + GRAVITY * sim_dt) * sim_dt;
        let substeps = (sim_dt * 120.0)
            .max(max_travel / travel_px)
            .ceil()
            .clamp(1.0, 32.0) as usize;
        let dt = sim_dt / substeps as f32;

        // Most sparks come from the same emitter and share its drag coefficient.
        let mut last_drag = f32::NAN;
        let mut damping = 1.0;
        for _ in 0..substeps {
            for p in &mut self.particles {
                if p.drag != last_drag {
                    last_drag = p.drag;
                    damping = p.drag.powf(dt * 60.0);
                }
                p.vx *= damping;
                p.vy = p.vy * damping + GRAVITY * dt;
                advance_through_keys(p, dt, sdf, geom);
                constrain_world(p);
            }

            // A pair can push a third spark (or the floor). Iterate to spread that pressure
            // through piles, reapplying the floor so separation can't push a spark below it.
            if self.particles.len() > 1 {
                for _ in 0..3 {
                    if !self.contacts.separate(&mut self.particles, cell_size) {
                        break;
                    }
                    for p in &mut self.particles {
                        constrain_world(p);
                    }
                }
            }
        }

        for p in &mut self.particles {
            let target_scale = if p.inside_key { p.lens_refract } else { 1.0 };
            p.current_scale += (target_scale - p.current_scale) * (dt_sec * 18.0).min(1.0);
        }
        self.particles.retain(|p| {
            // Keep a spark until the dot itself has cleared the visible edge.
            let margin = p.radius_px * p.current_scale;
            p.x > -margin && p.x < WIDTH + margin && p.y < HEIGHT + margin
        });
    }
}

fn advance_through_keys(p: &mut LiveParticle, dt: f32, sdf: &KeySdfGrid, geom: &KeyboardGeometry) {
    let x = p.x + p.vx * dt;
    let y = p.y + p.vy * dt;
    if p.has_landed || !(0.0..WIDTH).contains(&x) || !(0.0..HEIGHT).contains(&y) {
        p.x = x;
        p.y = y;
        p.inside_key = false;
        return;
    }

    let inside = sdf.sample(x, y).dist_px < 0.0;
    // A key is an open-top cup: entering never deflects a spark, and rising sparks can
    // leave freely. Sample the face from inside so a nearby key cannot supply the normal.
    if p.inside_key && !inside && p.vy >= 0.0 {
        let wall = sdf.sample(p.x, p.y);
        let cup = geom
            .key(wall.key_index)
            .is_some_and(|key| key.row < sdf.bottom_row.saturating_sub(1));
        if wall.dist_px < 0.0 && wall.ny >= -0.5 && cup {
            let outward_speed = p.vx * wall.nx + p.vy * wall.ny;
            let bottom = wall.ny > 0.5;
            // Skip low-speed bottom bounces so sparks don't settle inside the key.
            let threshold = if bottom { 125.0 } else { 0.0 };
            if outward_speed > threshold && p.next_rand01() < p.bounce_chance {
                let restitution = if bottom { 0.42 } else { 0.72 };
                let tangent_retention = 0.92;
                let normal_change = (tangent_retention + restitution) * outward_speed;
                p.vx = tangent_retention * p.vx - normal_change * wall.nx;
                p.vy = tangent_retention * p.vy - normal_change * wall.ny;
                return;
            }
        }
    }
    p.x = x;
    p.y = y;
    p.inside_key = inside;
}

fn constrain_world(p: &mut LiveParticle) {
    // Keep the soft top bounce, but leave both sides open.
    let ceiling = p.radius_px.max(4.0);
    if p.y < ceiling {
        p.y = ceiling;
        p.vy = p.vy.abs() * 0.7;
    }
    if !(0.0..=WIDTH).contains(&p.x) {
        return;
    }

    // Keep the middle of the pile in view, with two ramps that run below the screen at
    // the edges. Projecting gravity onto the ramps drains the pile toward the exits.
    const SLOPE: f32 = 0.044;
    const CREST_HALF_WIDTH: f32 = 36.0;
    const EDGE_Y: f32 = HEIGHT + 4.0;
    const CREST_Y: f32 = EDGE_Y - SLOPE * (WIDTH * 0.5 - CREST_HALF_WIDTH * 0.5);
    if p.y + p.radius_px * 1.001 <= CREST_Y {
        return;
    }
    let x_from_center = p.x - WIDTH * 0.5;
    let abs_x = x_from_center.abs();
    let (rise_from_crest, slope) = if abs_x < CREST_HALF_WIDTH {
        (
            0.5 * SLOPE * abs_x * abs_x / CREST_HALF_WIDTH,
            SLOPE * x_from_center / CREST_HALF_WIDTH,
        )
    } else {
        (
            SLOPE * (abs_x - CREST_HALF_WIDTH * 0.5),
            SLOPE * x_from_center.signum(),
        )
    };
    let surface_y = CREST_Y + rise_from_crest;
    let normal_len = (1.0 + slope * slope).sqrt();
    let contact_y = surface_y - p.radius_px * normal_len;
    if p.y > contact_y {
        // After the first floor contact only the floor and other sparks can deflect this one.
        p.has_landed = true;
        p.inside_key = false;
        p.y = contact_y;
        let nx = slope / normal_len;
        let ny = -1.0 / normal_len;
        let approach = p.vx * nx + p.vy * ny;
        if approach < 0.0 {
            let restitution = if approach < -REST_SPEED { 0.23 } else { 0.0 };
            p.vx -= (1.0 + restitution) * approach * nx;
            p.vy -= (1.0 + restitution) * approach * ny;
        }
    }
}

/// Reusable broad phase. Cells are at least one diameter wide, so each contact is either
/// within one cell or in an adjacent cell. Retaining buckets avoids per-step allocations.
#[derive(Default)]
struct ContactGrid {
    cells: Vec<Vec<usize>>,
    occupied: Vec<usize>,
}

impl ContactGrid {
    fn separate(&mut self, particles: &mut [LiveParticle], cell_size: f32) -> bool {
        let inv_cell = cell_size.recip();
        let cols = ((WIDTH * inv_cell).ceil() as usize).max(1);
        let rows = ((HEIGHT * inv_cell).ceil() as usize).max(1);
        let len = cols * rows;
        if self.cells.len() < len {
            self.cells.resize_with(len, Vec::new);
        }
        for idx in self.occupied.drain(..) {
            self.cells[idx].clear();
        }
        let cells = &mut self.cells[..len];
        for (i, p) in particles.iter().enumerate() {
            let x = ((p.x * inv_cell) as usize).min(cols - 1);
            let y = ((p.y * inv_cell) as usize).min(rows - 1);
            let idx = y * cols + x;
            if cells[idx].is_empty() {
                self.occupied.push(idx);
            }
            cells[idx].push(i);
        }

        let mut moved = false;
        for &idx in &self.occupied {
            let cell = &cells[idx];
            let (x, y) = (idx % cols, idx / cols);
            for (pos, &i) in cell.iter().enumerate() {
                for &j in &cell[pos + 1..] {
                    moved |= separate_pair(particles, i, j);
                }
            }
            // Only visit neighbors that come after this bucket, so each pair is solved once.
            if x + 1 < cols {
                for &i in cell {
                    for &j in &cells[idx + 1] {
                        moved |= separate_pair(particles, i, j);
                    }
                }
            }
            if y + 1 < rows {
                let below = (y + 1) * cols;
                for neighbor in &cells[below + x.saturating_sub(1)..below + (x + 2).min(cols)] {
                    for &i in cell {
                        for &j in neighbor {
                            moved |= separate_pair(particles, i, j);
                        }
                    }
                }
            }
        }
        moved
    }
}

fn separate_pair(particles: &mut [LiveParticle], i: usize, j: usize) -> bool {
    let dx = particles[j].x - particles[i].x;
    let dy = particles[j].y - particles[i].y;
    let min_dist = particles[i].radius_px + particles[j].radius_px;
    let dist_sq = dx * dx + dy * dy;
    if dist_sq >= min_dist * min_dist {
        return false;
    }
    let [a, b] = particles
        .get_disjoint_mut([i, j])
        .expect("distinct grid entries");
    let dist = dist_sq.sqrt();
    let rvx = b.vx - a.vx;
    let rvy = b.vy - a.vy;
    let (nx, ny) = if dist > 1e-4 {
        (dx / dist, dy / dist)
    } else {
        // Coincident spawns still need to separate, even at zero relative speed.
        let speed = rvx.hypot(rvy);
        if speed > 1e-4 {
            (-rvx / speed, -rvy / speed)
        } else {
            (1.0, 0.0)
        }
    };
    let correction = (min_dist - dist) * 0.5;
    a.x -= correction * nx;
    a.y -= correction * ny;
    b.x += correction * nx;
    b.y += correction * ny;

    let approach = rvx * nx + rvy * ny;
    if approach < 0.0 {
        let restitution = if approach < -REST_SPEED { 0.4 } else { 0.0 };
        let impulse = -(1.0 + restitution) * approach * 0.5;
        a.vx -= impulse * nx;
        a.vy -= impulse * ny;
        b.vx += impulse * nx;
        b.vy += impulse * ny;
    }
    true
}
