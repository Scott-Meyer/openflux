//! Crisp 2D software canvas rasterizer for module DrawCommands (tiles, key legends, world overlay)
//! producing premultiplied RGBA8 surfaces uploaded directly to the GPU compositor.

use fontdue::{Font, FontSettings};
use image::RgbaImage;
use openflux_bus::KeyboardGeometry;
use openflux_modules::{DrawCommand, TextAlign};
use std::collections::HashMap;
use std::path::Path;

pub struct Canvas2D {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8 pixels (`width * height * 4`).
    pub rgba: Vec<u8>,
    clip: Option<(i32, i32, i32, i32)>,
    font: Option<Font>,
    image_cache: HashMap<String, RgbaImage>,
}

impl Canvas2D {
    pub fn new(width: u32, height: u32, font_bytes: Option<&[u8]>) -> Self {
        let font = font_bytes.and_then(|b| Font::from_bytes(b, FontSettings::default()).ok());
        Self {
            width,
            height,
            rgba: vec![0u8; (width as usize) * (height as usize) * 4],
            clip: None,
            font,
            image_cache: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.rgba.fill(0);
        self.clip = None;
    }

    pub fn execute(&mut self, commands: &[DrawCommand]) {
        self.clip = None;
        for cmd in commands {
            match cmd {
                DrawCommand::Clip { x, y, w, h } => {
                    self.clip = Some((
                        (*x).floor() as i32,
                        (*y).floor() as i32,
                        (*x + *w).ceil() as i32,
                        (*y + *h).ceil() as i32,
                    ));
                }
                DrawCommand::ClearClip => self.clip = None,
                DrawCommand::Rect { x, y, w, h, color } => {
                    self.fill_round_rect(*x, *y, *w, *h, 0.0, *color, 0.0, [0.0; 4]);
                }
                DrawCommand::RoundRect {
                    x,
                    y,
                    w,
                    h,
                    radius,
                    color,
                    stroke_width,
                    stroke_color,
                } => {
                    self.fill_round_rect(
                        *x,
                        *y,
                        *w,
                        *h,
                        *radius,
                        *color,
                        *stroke_width,
                        *stroke_color,
                    );
                }
                DrawCommand::Circle {
                    cx,
                    cy,
                    r,
                    color,
                    stroke_width,
                    stroke_color,
                } => {
                    self.fill_circle(*cx, *cy, *r, *color, *stroke_width, *stroke_color);
                }
                DrawCommand::Ellipse {
                    cx,
                    cy,
                    rx,
                    ry,
                    color,
                } => {
                    self.fill_ellipse(*cx, *cy, *rx, *ry, *color);
                }
                DrawCommand::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    color,
                } => {
                    self.draw_line(*x1, *y1, *x2, *y2, *width, *color);
                }
                DrawCommand::Arc {
                    cx,
                    cy,
                    r,
                    start_deg,
                    end_deg,
                    width,
                    color,
                } => {
                    self.draw_arc(*cx, *cy, *r, *start_deg, *end_deg, *width, *color);
                }
                DrawCommand::Text {
                    x,
                    y,
                    text,
                    size,
                    color,
                    align,
                } => {
                    self.draw_text(*x, *y, text, *size, *color, *align);
                }
                DrawCommand::Image {
                    x,
                    y,
                    w,
                    h,
                    path,
                    tint,
                } => {
                    self.draw_image(*x, *y, *w, *h, path, *tint);
                }
            }
        }
        self.clip = None;
    }

    /// Draws subtle keycap frame outlines and ANSI/ISO key legends so legends stay legible over
    /// intense refraction and particle layers.
    pub fn draw_default_key_legends(&mut self, geom: &KeyboardGeometry, labels: &[&str]) {
        for key in &geom.keys {
            let r = &key.rect;
            // Subtle keycap rim outline so physical key boundaries are visually crisp in preview.
            self.fill_round_rect(
                r.x,
                r.y,
                r.w,
                r.h,
                key.corner_radius_px,
                [0.02, 0.03, 0.05, 0.14],
                1.2,
                [0.85, 0.92, 1.0, 0.18],
            );
            if let Some(&label) = labels.get(key.key_index as usize) {
                if !label.is_empty() {
                    let font_size = if label.len() <= 2 {
                        30.0
                    } else if label.len() <= 5 {
                        20.0
                    } else {
                        16.0
                    };
                    // Soft drop shadow for legibility over bright backgrounds/water highlights.
                    self.draw_text(
                        key.center_x + 1.5,
                        key.center_y - font_size * 0.42 + 1.5,
                        label,
                        font_size,
                        [0.0, 0.0, 0.0, 0.72],
                        TextAlign::Center,
                    );
                    self.draw_text(
                        key.center_x,
                        key.center_y - font_size * 0.42,
                        label,
                        font_size,
                        [0.95, 0.97, 1.0, 0.94],
                        TextAlign::Center,
                    );
                }
            }
        }
    }

    fn bounds(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> (i32, i32, i32, i32) {
        let mut min_x = (x0.floor() as i32).max(0);
        let mut min_y = (y0.floor() as i32).max(0);
        let mut max_x = (x1.ceil() as i32).min(self.width as i32);
        let mut max_y = (y1.ceil() as i32).min(self.height as i32);
        if let Some((cx0, cy0, cx1, cy1)) = self.clip {
            min_x = min_x.max(cx0);
            min_y = min_y.max(cy0);
            max_x = max_x.min(cx1);
            max_y = max_y.min(cy1);
        }
        (min_x, min_y, max_x, max_y)
    }

    #[inline]
    fn blend_px(&mut self, x: i32, y: i32, color: [f32; 4], cov: f32) {
        let a = (color[3] * cov).clamp(0.0, 1.0);
        if a <= 0.001 {
            return;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let dst_a = self.rgba[idx + 3] as f32 / 255.0;
        let out_a = a + dst_a * (1.0 - a);
        if out_a <= 1e-5 {
            return;
        }
        let sr = color[0].clamp(0.0, 1.0);
        let sg = color[1].clamp(0.0, 1.0);
        let sb = color[2].clamp(0.0, 1.0);

        let dr = self.rgba[idx] as f32 / 255.0;
        let dg = self.rgba[idx + 1] as f32 / 255.0;
        let db = self.rgba[idx + 2] as f32 / 255.0;

        let inv = 1.0 - a;
        let or = (sr * a + dr * dst_a * inv) / out_a;
        let og = (sg * a + dg * dst_a * inv) / out_a;
        let ob = (sb * a + db * dst_a * inv) / out_a;

        self.rgba[idx] = (or * 255.0).round().clamp(0.0, 255.0) as u8;
        self.rgba[idx + 1] = (og * 255.0).round().clamp(0.0, 255.0) as u8;
        self.rgba[idx + 2] = (ob * 255.0).round().clamp(0.0, 255.0) as u8;
        self.rgba[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_round_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: [f32; 4],
        stroke_width: f32,
        stroke_color: [f32; 4],
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let pad = stroke_width.max(0.0) + 2.0;
        let (x0, y0, x1, y1) = self.bounds(x - pad, y - pad, x + w + pad, y + h + pad);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
        let hx = w * 0.5 - r;
        let hy = h * 0.5 - r;
        let do_fill = color[3] > 0.001;
        let do_stroke = stroke_width > 0.1 && stroke_color[3] > 0.001;

        for py in y0..y1 {
            let fy = py as f32 + 0.5;
            let dy = (fy - cy).abs() - hy;
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let dx = (fx - cx).abs() - hx;
                let d = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r;
                if do_fill {
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_px(px, py, color, cov);
                    }
                }
                if do_stroke {
                    let half = stroke_width * 0.5;
                    let sd = (d.abs() - half).max(0.0);
                    let scov = (1.0 - sd).clamp(0.0, 1.0);
                    if scov > 0.0 {
                        self.blend_px(px, py, stroke_color, scov);
                    }
                }
            }
        }
    }

    fn fill_circle(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        color: [f32; 4],
        stroke_width: f32,
        stroke_color: [f32; 4],
    ) {
        if r <= 0.0 {
            return;
        }
        let pad = r + stroke_width + 2.0;
        let (x0, y0, x1, y1) = self.bounds(cx - pad, cy - pad, cx + pad, cy + pad);
        let do_fill = color[3] > 0.001;
        let do_stroke = stroke_width > 0.1 && stroke_color[3] > 0.001;
        for py in y0..y1 {
            let dy = py as f32 + 0.5 - cy;
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let d = (dx * dx + dy * dy).sqrt() - r;
                if do_fill {
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_px(px, py, color, cov);
                    }
                }
                if do_stroke {
                    let sd = (d.abs() - stroke_width * 0.5).max(0.0);
                    let scov = (1.0 - sd).clamp(0.0, 1.0);
                    if scov > 0.0 {
                        self.blend_px(px, py, stroke_color, scov);
                    }
                }
            }
        }
    }

    fn fill_ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, color: [f32; 4]) {
        if rx <= 0.5 || ry <= 0.5 || color[3] <= 0.001 {
            return;
        }
        let (x0, y0, x1, y1) = self.bounds(cx - rx - 2.0, cy - ry - 2.0, cx + rx + 2.0, cy + ry + 2.0);
        let inv_rx = 1.0 / rx;
        let inv_ry = 1.0 / ry;
        let min_r = rx.min(ry);
        for py in y0..y1 {
            let ny = (py as f32 + 0.5 - cy) * inv_ry;
            for px in x0..x1 {
                let nx = (px as f32 + 0.5 - cx) * inv_rx;
                let norm_d = (nx * nx + ny * ny).sqrt();
                let d_px = (norm_d - 1.0) * min_r;
                let cov = (0.5 - d_px).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, color, cov);
                }
            }
        }
    }

    fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: [f32; 4]) {
        if width <= 0.0 || color[3] <= 0.001 {
            return;
        }
        let half = width * 0.5;
        let pad = half + 2.0;
        let (bx0, by0, bx1, by1) =
            self.bounds(x1.min(x2) - pad, y1.min(y2) - pad, x1.max(x2) + pad, y1.max(y2) + pad);
        let vx = x2 - x1;
        let vy = y2 - y1;
        let len2 = (vx * vx + vy * vy).max(1e-5);
        for py in by0..by1 {
            let fy = py as f32 + 0.5;
            for px in bx0..bx1 {
                let fx = px as f32 + 0.5;
                let t = (((fx - x1) * vx + (fy - y1) * vy) / len2).clamp(0.0, 1.0);
                let dx = fx - (x1 + vx * t);
                let dy = fy - (y1 + vy * t);
                let d = (dx * dx + dy * dy).sqrt() - half;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, color, cov);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_arc(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        start_deg: f32,
        end_deg: f32,
        width: f32,
        color: [f32; 4],
    ) {
        let sweep = end_deg - start_deg;
        if sweep.abs() < 0.1 || r <= 0.5 {
            return;
        }
        let steps = ((sweep.abs() / 6.0).ceil() as usize).clamp(4, 96);
        let mut prev_x = cx + r * start_deg.to_radians().cos();
        let mut prev_y = cy + r * start_deg.to_radians().sin();
        for i in 1..=steps {
            let deg = start_deg + sweep * (i as f32 / steps as f32);
            let nx = cx + r * deg.to_radians().cos();
            let ny = cy + r * deg.to_radians().sin();
            self.draw_line(prev_x, prev_y, nx, ny, width, color);
            prev_x = nx;
            prev_y = ny;
        }
    }

    fn draw_text(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: [f32; 4],
        align: TextAlign,
    ) {
        let Some(font) = self.font.as_ref() else {
            return;
        };
        if text.is_empty() || size <= 1.0 || color[3] <= 0.001 {
            return;
        }
        let glyphs: Vec<(fontdue::Metrics, Vec<u8>)> = text
            .chars()
            .map(|ch| font.rasterize(ch, size))
            .collect();
        let total_w: f32 = glyphs.iter().map(|(m, _)| m.advance_width).sum();
        let start_x = match align {
            TextAlign::Left => x,
            TextAlign::Center => x - total_w * 0.5,
            TextAlign::Right => x - total_w,
        };
        let baseline_y = y + size * 0.82;
        let mut pen_x = start_x;

        for (m, bitmap) in glyphs {
            let gx = (pen_x + m.xmin as f32).round() as i32;
            let gy = (baseline_y - m.height as f32 - m.ymin as f32).round() as i32;
            for row in 0..m.height {
                let py = gy + row as i32;
                if py < 0 || py >= self.height as i32 {
                    continue;
                }
                if let Some((_, cy0, _, cy1)) = self.clip {
                    if py < cy0 || py >= cy1 {
                        continue;
                    }
                }
                for col in 0..m.width {
                    let px = gx + col as i32;
                    if px < 0 || px >= self.width as i32 {
                        continue;
                    }
                    if let Some((cx0, _, cx1, _)) = self.clip {
                        if px < cx0 || px >= cx1 {
                            continue;
                        }
                    }
                    let cov = bitmap[row * m.width + col] as f32 / 255.0;
                    if cov > 0.01 {
                        self.blend_px(px, py, color, cov);
                    }
                }
            }
            pen_x += m.advance_width;
        }
    }

    fn draw_image(&mut self, x: f32, y: f32, w: f32, h: f32, path: &str, tint: [f32; 4]) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        if !self.image_cache.contains_key(path) {
            if let Ok(img) = image::open(Path::new(path)) {
                self.image_cache.insert(path.to_string(), img.to_rgba8());
            } else {
                return;
            }
        }
        let Some(img) = self.image_cache.get(path).cloned() else {
            return;
        };
        let (x0, y0, x1, y1) = self.bounds(x, y, x + w, y + h);
        let iw = img.width() as f32;
        let ih = img.height() as f32;
        for py in y0..y1 {
            let v = ((py as f32 - y) / h).clamp(0.0, 0.9999);
            let sy = (v * ih) as u32;
            for px in x0..x1 {
                let u = ((px as f32 - x) / w).clamp(0.0, 0.9999);
                let sx = (u * iw) as u32;
                let p = img.get_pixel(sx, sy).0;
                let col = [
                    (p[0] as f32 / 255.0) * tint[0],
                    (p[1] as f32 / 255.0) * tint[1],
                    (p[2] as f32 / 255.0) * tint[2],
                    (p[3] as f32 / 255.0) * tint[3],
                ];
                self.blend_px(px, py, col, 1.0);
            }
        }
    }
}

/// Default ANSI 84-key legends for clean preview readability when no custom keymap labels are given.
pub const DEFAULT_ANSI_LABELS: &[&str] = &[
    // Row 0 (16)
    "ESC", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "PRT", "INS", "DEL",
    // Row 1 (15)
    "`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "=", "BKSP", "HOME",
    // Row 2 (15)
    "TAB", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "[", "]", "\\", "PGUP",
    // Row 3 (14)
    "CAPS", "A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'", "ENTER", "PGDN",
    // Row 4 (14)
    "SHIFT", "Z", "X", "C", "V", "B", "N", "M", ",", ".", "/", "SHIFT", "UP", "END",
    // Row 5 (11)
    "CTRL", "OPT", "CMD", "FN", "SPACE", "CMD", "ALT", "CTRL", "LEFT", "DOWN", "RIGHT",
];
