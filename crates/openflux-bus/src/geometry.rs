//! Physical geometry of the Flux keyboard display, keycaps, inter-key trenches, and top-bar slots.
//!
//! Coordinates are exposed in both display pixels (`1920x1080`) and physical millimeters
//! (`309.31mm x 174.0mm`) so visual modules, physics/particle simulations, and roaming companions
//! (such as the Idle Pet) can reason about exact physical keycap positions across ANSI and ISO frames.

use serde::{Deserialize, Serialize};

pub const DISPLAY_WIDTH_PX: u32 = 1920;
pub const DISPLAY_HEIGHT_PX: u32 = 1080;
pub const DISPLAY_WIDTH_MM: f32 = 309.31;
pub const PX_PER_MM: f32 = DISPLAY_WIDTH_PX as f32 / DISPLAY_WIDTH_MM;
pub const DISPLAY_HEIGHT_MM: f32 = DISPLAY_HEIGHT_PX as f32 / PX_PER_MM;

pub const KEY_HEIGHT_MM: f32 = 17.0;
pub const KEY_GAP_MM: f32 = 2.0;
pub const F_ROW_GAP_MM: f32 = 6.75;
pub const BOTTOM_MARGIN_MM: f32 = 3.07;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameLayout {
    Ansi,
    Iso,
}

impl Default for FrameLayout {
    fn default() -> Self {
        Self::Ansi
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RectPx {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectPx {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    /// Signed distance to a rounded rectangle with corner radius `r` (negative inside, positive outside).
    pub fn signed_distance_rounded(&self, px: f32, py: f32, radius: f32) -> f32 {
        let (cx, cy) = self.center();
        let r = radius.min(self.w * 0.5).min(self.h * 0.5).max(0.0);
        let hx = self.w * 0.5 - r;
        let hy = self.h * 0.5 - r;
        let dx = (px - cx).abs() - hx;
        let dy = (py - cy).abs() - hy;
        let out_dist = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
        let in_dist = dx.max(dy).min(0.0);
        out_dist + in_dist - r
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KeyGeometry {
    pub key_index: u16,
    pub row: u8,
    pub col: u8,
    pub rect: RectPx,
    pub center_x: f32,
    pub center_y: f32,
    /// Parallax-shifted visual center in display pixels (defaults to `center_x`/`center_y`).
    #[serde(default)]
    pub visual_x: f32,
    #[serde(default)]
    pub visual_y: f32,
    /// Parallax-shifted apparent keycap rectangle in display pixels so any module or physics
    /// system (legends, key-bouncing particles, effects, companions) hits true optical borders.
    #[serde(default)]
    pub visual_rect: Option<RectPx>,
    #[serde(default)]
    pub parallax_dx: f32,
    #[serde(default)]
    pub parallax_dy: f32,
    /// Physical center in millimeters from top-left of the active display.
    pub center_mm_x: f32,
    pub center_mm_y: f32,
    /// Normalized display coordinates `[0.0, 1.0]`.
    pub uv_x: f32,
    pub uv_y: f32,
    /// Rounded keycap corner radius in pixels.
    pub corner_radius_px: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SlotGeometry {
    pub slot_index: u8,
    pub rect: RectPx,
    pub center_x: f32,
    pub center_y: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyboardGeometry {
    pub layout: FrameLayout,
    pub width_px: u32,
    pub height_px: u32,
    pub width_mm: f32,
    pub height_mm: f32,
    pub px_per_mm: f32,
    pub keys: Vec<KeyGeometry>,
    pub slots: Vec<SlotGeometry>,
    pub header_rect: RectPx,
    pub key_area_rect: RectPx,
}

/// Result of sampling the physical keycap signed distance field at `(x_px, y_px)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeySdfSample {
    /// Signed distance in pixels (negative inside a keycap, positive in the 2mm trenches/margins).
    pub signed_distance_px: f32,
    /// Outward surface normal pointing away from the nearest keycap center.
    pub normal_x: f32,
    pub normal_y: f32,
    /// Index of the nearest keycap in `KeyboardGeometry::keys`.
    pub nearest_key_index: u16,
}

fn push_row_mm(
    keys: &mut Vec<KeyGeometry>,
    row: u8,
    widths_mm: &[f32],
    y_px: f32,
    key_h_px: f32,
) {
    let n = widths_mm.len();
    let row_w_mm: f32 =
        widths_mm.iter().sum::<f32>() + if n > 0 { (n - 1) as f32 * KEY_GAP_MM } else { 0.0 };
    let mut x_mm = (DISPLAY_WIDTH_MM - row_w_mm) * 0.5;
    let corner_radius_px = 1.6 * PX_PER_MM;

    for (col, &w_mm) in widths_mm.iter().enumerate() {
        if w_mm > 0.0 {
            let idx = keys.len() as u16;
            let rect = RectPx {
                x: x_mm * PX_PER_MM,
                y: y_px,
                w: w_mm * PX_PER_MM,
                h: key_h_px,
            };
            let (cx, cy) = rect.center();
            keys.push(KeyGeometry {
                key_index: idx,
                row,
                col: col as u8,
                rect,
                center_x: cx,
                center_y: cy,
                visual_x: cx,
                visual_y: cy,
                visual_rect: Some(rect),
                parallax_dx: 0.0,
                parallax_dy: 0.0,
                center_mm_x: cx / PX_PER_MM,
                center_mm_y: cy / PX_PER_MM,
                uv_x: cx / DISPLAY_WIDTH_PX as f32,
                uv_y: cy / DISPLAY_HEIGHT_PX as f32,
                corner_radius_px,
            });
            x_mm += w_mm + KEY_GAP_MM;
        }
    }
}

impl KeyboardGeometry {
    pub fn build(layout: FrameLayout) -> Self {
        let key_h_px = KEY_HEIGHT_MM * PX_PER_MM;
        let row_gaps_below_mm = [F_ROW_GAP_MM, KEY_GAP_MM, KEY_GAP_MM, KEY_GAP_MM, KEY_GAP_MM, BOTTOM_MARGIN_MM];

        let mut row_y_px = [0.0_f32; 6];
        let mut y_bottom_mm = DISPLAY_HEIGHT_PX as f32 / PX_PER_MM;
        for r in (0..6).rev() {
            y_bottom_mm -= row_gaps_below_mm[r];
            let y_top_mm = y_bottom_mm - KEY_HEIGHT_MM;
            row_y_px[r] = y_top_mm * PX_PER_MM;
            y_bottom_mm = y_top_mm;
        }

        let mosaic_margin_x = 8.1_f32;
        let mosaic_x = mosaic_margin_x;
        let mosaic_w = DISPLAY_WIDTH_PX as f32 - 2.0 * mosaic_margin_x;
        let mosaic_y = 5.0_f32;
        let total_kb_h_px = (DISPLAY_HEIGHT_PX as f32 - (y_bottom_mm * PX_PER_MM)).max(600.0);
        let avail_header_h = (DISPLAY_HEIGHT_PX as f32 - total_kb_h_px).max(200.0);
        let mosaic_h = (avail_header_h - 80.0_f32).max(160.0);
        let mosaic_gap = 89.4_f32;

        let mut keys = Vec::with_capacity(90);
        let row0 = [17.0_f32; 16];
        let row1 = [
            17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 36.0,
            17.0,
        ];
        push_row_mm(&mut keys, 0, &row0, row_y_px[0], key_h_px);
        push_row_mm(&mut keys, 1, &row1, row_y_px[1], key_h_px);

        if layout == FrameLayout::Iso {
            let row2 = [
                26.53, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
                26.53, 17.0,
            ];
            let row3 = [
                31.29, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
                21.77, 17.0,
            ];
            let row4 = [
                21.77, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 31.29,
                17.0, 17.0,
            ];
            let row5 = [
                21.77, 17.0, 17.0, 17.0, 107.49, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
            ];
            push_row_mm(&mut keys, 2, &row2, row_y_px[2], key_h_px);
            push_row_mm(&mut keys, 3, &row3, row_y_px[3], key_h_px);
            push_row_mm(&mut keys, 4, &row4, row_y_px[4], key_h_px);
            push_row_mm(&mut keys, 5, &row5, row_y_px[5], key_h_px);
        } else {
            let row2 = [
                26.53, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
                26.53, 17.0,
            ];
            let row3 = [
                31.29, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 40.82,
                17.0,
            ];
            let row4 = [
                40.82, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 31.29, 17.0,
                17.0,
            ];
            let row5 = [
                21.77, 17.0, 17.0, 17.0, 107.49, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
            ];
            push_row_mm(&mut keys, 2, &row2, row_y_px[2], key_h_px);
            push_row_mm(&mut keys, 3, &row3, row_y_px[3], key_h_px);
            push_row_mm(&mut keys, 4, &row4, row_y_px[4], key_h_px);
            push_row_mm(&mut keys, 5, &row5, row_y_px[5], key_h_px);
        }

        let header_rect = RectPx {
            x: mosaic_x,
            y: mosaic_y,
            w: mosaic_w,
            h: mosaic_h,
        };
        let slot_w = (mosaic_w - 3.0 * mosaic_gap) / 4.0;
        let slots = (0..4u8)
            .map(|i| {
                let rect = RectPx {
                    x: mosaic_x + i as f32 * (slot_w + mosaic_gap),
                    y: mosaic_y,
                    w: slot_w,
                    h: mosaic_h,
                };
                let (cx, cy) = rect.center();
                SlotGeometry {
                    slot_index: i,
                    rect,
                    center_x: cx,
                    center_y: cy,
                }
            })
            .collect();

        let min_x = keys.iter().map(|k| k.rect.x).fold(f32::INFINITY, f32::min);
        let max_x = keys
            .iter()
            .map(|k| k.rect.x + k.rect.w)
            .fold(0.0_f32, f32::max);
        let min_y = keys.iter().map(|k| k.rect.y).fold(f32::INFINITY, f32::min);
        let max_y = keys
            .iter()
            .map(|k| k.rect.y + k.rect.h)
            .fold(0.0_f32, f32::max);

        Self {
            layout,
            width_px: DISPLAY_WIDTH_PX,
            height_px: DISPLAY_HEIGHT_PX,
            width_mm: DISPLAY_WIDTH_MM,
            height_mm: DISPLAY_HEIGHT_MM,
            px_per_mm: PX_PER_MM,
            keys,
            slots,
            header_rect,
            key_area_rect: RectPx {
                x: min_x,
                y: min_y,
                w: (max_x - min_x).max(0.0),
                h: (max_y - min_y).max(0.0),
            },
        }
    }

    pub fn key(&self, key_index: u16) -> Option<&KeyGeometry> {
        self.keys.get(key_index as usize)
    }

    pub fn slot(&self, slot_index: u8) -> Option<&SlotGeometry> {
        self.slots.get(slot_index as usize)
    }

    /// Applies optical parallax offset (matching physical eye-to-keycap projection through the clear
    /// keyframe) to every key's `visual_x`/`visual_y` and `parallax_dx`/`parallax_dy`, so any module
    /// consuming `keyboard/geometry` automatically aligns legends, icons, and overlays to the viewer.
    pub fn apply_parallax(
        &mut self,
        enabled: bool,
        eye_x_mm: f32,
        eye_y_mm: f32,
        eye_z_mm: f32,
        glass_gap_mm: f32,
        global_dx_px: f32,
        global_dy_px: f32,
    ) {
        let z = eye_z_mm.max(120.0);
        let gap = glass_gap_mm.clamp(0.0, 20.0);
        let screen_cx_mm = DISPLAY_WIDTH_MM * 0.5;
        let screen_cy_mm = DISPLAY_HEIGHT_MM * 0.5;
        for k in &mut self.keys {
            if enabled {
                let rel_x_mm = (k.center_mm_x - screen_cx_mm) - eye_x_mm;
                let rel_y_mm = (k.center_mm_y - screen_cy_mm) - eye_y_mm;
                let dx_px = (rel_x_mm * (gap / z) * PX_PER_MM).clamp(-24.0, 24.0) + global_dx_px;
                let dy_px = (rel_y_mm * (gap / z) * PX_PER_MM).clamp(-24.0, 24.0) + global_dy_px;
                k.parallax_dx = dx_px;
                k.parallax_dy = dy_px;
                k.visual_x = k.center_x + dx_px;
                k.visual_y = k.center_y + dy_px;
                k.visual_rect = Some(RectPx {
                    x: k.rect.x + dx_px,
                    y: k.rect.y + dy_px,
                    w: k.rect.w,
                    h: k.rect.h,
                });
            } else {
                k.parallax_dx = global_dx_px;
                k.parallax_dy = global_dy_px;
                k.visual_x = k.center_x + global_dx_px;
                k.visual_y = k.center_y + global_dy_px;
                k.visual_rect = Some(RectPx {
                    x: k.rect.x + global_dx_px,
                    y: k.rect.y + global_dy_px,
                    w: k.rect.w,
                    h: k.rect.h,
                });
            }
        }
    }

    /// Finds the keycap containing `(px, py)`, or the nearest keycap center if in a trench.
    pub fn nearest_key(&self, px: f32, py: f32) -> Option<&KeyGeometry> {
        self.keys.iter().min_by(|a, b| {
            let da = (a.center_x - px).powi(2) + (a.center_y - py).powi(2);
            let db = (b.center_x - px).powi(2) + (b.center_y - py).powi(2);
            da.total_cmp(&db)
        })
    }

    /// Evaluates the signed distance field across all keycaps at `(px, py)`.
    pub fn sample_key_sdf(&self, px: f32, py: f32) -> KeySdfSample {
        let mut best_dist = f32::INFINITY;
        let mut best_key = 0u16;
        for k in &self.keys {
            let d = k.effective_visual_rect().signed_distance_rounded(px, py, k.corner_radius_px);
            if d < best_dist {
                best_dist = d;
                best_key = k.key_index;
            }
        }
        // Central-difference gradient for smooth outward normal at corners and trenches.
        let eps = 1.0_f32;
        let d_px = self.raw_min_sdf(px + eps, py);
        let d_mx = self.raw_min_sdf(px - eps, py);
        let d_py = self.raw_min_sdf(px, py + eps);
        let d_my = self.raw_min_sdf(px, py - eps);
        let gx = d_px - d_mx;
        let gy = d_py - d_my;
        let len = (gx * gx + gy * gy).sqrt();
        let (nx, ny) = if len > 1e-5 {
            (gx / len, gy / len)
        } else {
            (0.0, -1.0)
        };
        KeySdfSample {
            signed_distance_px: best_dist,
            normal_x: nx,
            normal_y: ny,
            nearest_key_index: best_key,
        }
    }

    fn raw_min_sdf(&self, px: f32, py: f32) -> f32 {
        let mut best = f32::INFINITY;
        for k in &self.keys {
            let d = k.effective_visual_rect().signed_distance_rounded(px, py, k.corner_radius_px);
            if d < best {
                best = d;
            }
        }
        best
    }
}

impl KeyGeometry {
    /// Returns the parallax-shifted apparent keycap rectangle in display pixels.
    pub fn effective_visual_rect(&self) -> RectPx {
        if let Some(vr) = self.visual_rect {
            vr
        } else {
            RectPx {
                x: self.rect.x + self.parallax_dx,
                y: self.rect.y + self.parallax_dy,
                w: self.rect.w,
                h: self.rect.h,
            }
        }
    }
}
