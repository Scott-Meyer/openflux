//! Optical parallax math & display dimensions for Keyglass v2.
//!
//! Computes Snell's-law optical refraction through the 9.0 mm acrylic keycaps over the
//! 1920x1080 panel so legends appear optically centered on each key from the user's seated eye position.

pub const DISPLAY_WIDTH: i32 = 1920;
pub const DISPLAY_HEIGHT: i32 = 1080;

#[derive(Debug, Clone, PartialEq)]
pub struct ParallaxConfig {
    pub parallax_active: bool,
    pub eye_height_mm: f64,
    pub eye_dist_front_mm: f64,
    pub eye_x_offset_mm: f64,
    pub foot_stand_deg: f64,
    pub display_offset_x_px: f64,
    pub display_offset_y_px: f64,
}

impl Default for ParallaxConfig {
    fn default() -> Self {
        Self {
            parallax_active: true,
            eye_height_mm: 425.0,
            eye_dist_front_mm: 300.0,
            eye_x_offset_mm: 0.0,
            foot_stand_deg: 0.0,
            display_offset_x_px: 0.0,
            display_offset_y_px: 0.0,
        }
    }
}

impl ParallaxConfig {
    pub fn from_json(val: &serde_json::Value) -> Self {
        let mut s = Self::default();
        if let Some(b) = val.get("parallaxActive").and_then(|v| v.as_bool()) {
            s.parallax_active = b;
        }
        if let Some(d) = val.get("eyeHeight").and_then(|v| v.as_f64()) {
            s.eye_height_mm = d;
        }
        if let Some(d) = val.get("eyeDistanceFromFront").and_then(|v| v.as_f64()) {
            s.eye_dist_front_mm = d;
        }
        if let Some(m) = val.get("footStandMode").and_then(|v| v.as_str()) {
            s.foot_stand_deg = match m {
                "low" => 4.0,
                "high" => 8.0,
                _ => 0.0,
            };
        }
        if let Some(x) = val.get("displayOffsetX").and_then(|v| v.as_f64()) {
            s.display_offset_x_px = x;
        }
        if let Some(y) = val.get("displayOffsetY").and_then(|v| v.as_f64()) {
            s.display_offset_y_px = y;
        }
        s
    }
}

pub fn compute_key_parallax(
    settings: &ParallaxConfig,
    key_cx_px: f64,
    key_cy_px: f64,
) -> (f64, f64) {
    const FRAME_W_UNITS: f64 = 309.31;
    const FRAME_H_UNITS: f64 = 121.82;
    const PHYS_W_MM: f64 = 307.0;
    const PHYS_H_MM: f64 = 123.0;
    const KEYCAP_THICKNESS_MM: f64 = 9.0;
    const REFRACTIVE_INDEX: f64 = 1.51;
    const MAX_OFFSET_PX: f64 = 28.0;

    let base_dx = settings.display_offset_x_px;
    let base_dy = settings.display_offset_y_px;
    if !settings.parallax_active || settings.eye_height_mm <= 10.0 {
        return (base_dx, base_dy);
    }

    let px_per_unit = DISPLAY_WIDTH as f64 / FRAME_W_UNITS;
    let key_x_units = (key_cx_px - (DISPLAY_WIDTH as f64) * 0.5) / px_per_unit;
    let key_y_from_front_units =
        ((DISPLAY_HEIGHT as f64 - key_cy_px) / px_per_unit).clamp(0.0, FRAME_H_UNITS);
    let key_x_mm = key_x_units * (PHYS_W_MM / FRAME_W_UNITS);
    let key_y_from_front_mm = key_y_from_front_units * (PHYS_H_MM / FRAME_H_UNITS);

    let tilt_rad = settings.foot_stand_deg.to_radians();
    let c = tilt_rad.cos();
    let s = tilt_rad.sin();
    let eye_x_mm = settings.eye_x_offset_mm;
    let eye_dist_front_mm = settings.eye_dist_front_mm;
    let eye_height_mm = settings.eye_height_mm;

    let rel_y = eye_dist_front_mm * c + eye_height_mm * s;
    let rel_z = eye_height_mm * c - eye_dist_front_mm * s;
    if rel_z <= 10.0 {
        return (base_dx, base_dy);
    }

    let x_dist = key_x_mm - eye_x_mm;
    let y_dist = key_y_from_front_mm + rel_y;
    let r_horiz = x_dist.hypot(y_dist);
    if r_horiz < 1e-6 {
        return (base_dx, base_dy);
    }

    let r_3d = r_horiz.hypot(rel_z);
    let sin_air = r_horiz / r_3d;
    let sin_keycap = (sin_air / REFRACTIVE_INDEX).clamp(-0.999, 0.999);
    let tan_keycap = sin_keycap / (1.0 - sin_keycap * sin_keycap).sqrt();
    let tan_air = r_horiz / rel_z;

    let shift_x_mm = (tan_air - tan_keycap) * KEYCAP_THICKNESS_MM * (x_dist / r_horiz);
    let shift_y_mm = -(tan_keycap * KEYCAP_THICKNESS_MM) * (y_dist / r_horiz);

    let mut dx = shift_x_mm * (DISPLAY_WIDTH as f64 / PHYS_W_MM);
    let mut dy = shift_y_mm * px_per_unit * (FRAME_H_UNITS / PHYS_H_MM);
    let mag = dx.hypot(dy);
    if mag > MAX_OFFSET_PX {
        let clamp = MAX_OFFSET_PX / mag;
        dx *= clamp;
        dy *= clamp;
    }
    (base_dx + dx, base_dy + dy)
}
