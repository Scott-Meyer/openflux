//! Crisp 2D software canvas rasterizer for module DrawCommands (tiles, key legends, world overlay)
//! producing premultiplied RGBA8 surfaces uploaded directly to the GPU compositor.

use fontdue::{Font, FontSettings};
use image::RgbaImage;
use openflux_bus::KeyboardGeometry;
use openflux_modules::{DrawCommand, TextAlign};
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

pub struct Canvas2D {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8 pixels (`width * height * 4`).
    pub rgba: Vec<u8>,
    is_clean: bool,
    cached_legends: Option<Vec<u8>>,
    clip: Option<(i32, i32, i32, i32)>,
    font: Option<Font>,
    glyph_cache: HashMap<(char, u16), (fontdue::Metrics, Vec<u8>)>,
    image_cache: HashMap<String, (Option<SystemTime>, RgbaImage)>,
}

impl Canvas2D {
    pub fn new(width: u32, height: u32, font_bytes: Option<&[u8]>) -> Self {
        let font = font_bytes.and_then(|b| Font::from_bytes(b, FontSettings::default()).ok());
        Self {
            width,
            height,
            rgba: vec![0u8; (width as usize) * (height as usize) * 4],
            is_clean: true,
            cached_legends: None,
            clip: None,
            font,
            glyph_cache: HashMap::new(),
            image_cache: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        if !self.is_clean {
            self.rgba.fill(0);
            self.is_clean = true;
        }
        self.clip = None;
    }

    /// Blits the cached keycap rims + legends layer (building it once on first call).
    pub fn blit_cached_default_key_legends(&mut self, geom: &KeyboardGeometry, labels: &[&str]) {
        if self.cached_legends.is_none() {
            let saved = std::mem::replace(
                &mut self.rgba,
                vec![0u8; (self.width as usize) * (self.height as usize) * 4],
            );
            let prev_clean = self.is_clean;
            self.is_clean = true;
            self.draw_default_key_legends(geom, labels);
            let baked = std::mem::replace(&mut self.rgba, saved);
            self.is_clean = prev_clean;
            self.cached_legends = Some(baked);
        }
        if let Some(ref baked) = self.cached_legends {
            if self.is_clean {
                self.rgba.copy_from_slice(baked);
            } else {
                for (dst, src) in self.rgba.chunks_exact_mut(4).zip(baked.chunks_exact(4)) {
                    let sa = src[3] as u32;
                    if sa == 0 {
                        continue;
                    }
                    if sa == 255 || dst[3] == 0 {
                        dst.copy_from_slice(src);
                        continue;
                    }
                    let inv = 255 - sa;
                    let da = dst[3] as u32;
                    let out_a = sa + ((da * inv + 127) / 255);
                    if out_a == 0 {
                        continue;
                    }
                    for c in 0..3 {
                        let num = src[c] as u32 * sa + ((dst[c] as u32 * da * inv + 127) / 255);
                        dst[c] = (num / out_a).min(255) as u8;
                    }
                    dst[3] = out_a.min(255) as u8;
                }
            }
            self.is_clean = false;
        }
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

    /// Draws only the subtle vertical hairline dividers between the 4 top-bar module slots.
    pub fn draw_top_bar_dividers(&mut self, geom: &KeyboardGeometry) {
        for i in 0..geom.slots.len().saturating_sub(1) {
            let a = &geom.slots[i].rect;
            let b = &geom.slots[i + 1].rect;
            let div_x = ((a.x + a.w) + b.x) * 0.5;
            let y0 = a.y + 18.0;
            let h = (a.h - 36.0).max(20.0);
            self.fill_round_rect(
                div_x,
                y0,
                1.2,
                h,
                0.0,
                [1.0, 1.0, 1.0, 0.16],
                0.0,
                [0.0; 4],
            );
        }
    }

    /// Draws clean floating key legends (fallback only when `keys-renderer` module isn't loaded)
    /// and subtle vertical hairline separators between the 4 top-bar module slots.
    pub fn draw_default_key_legends(&mut self, geom: &KeyboardGeometry, labels: &[&str]) {
        self.draw_top_bar_dividers(geom);

        for key in &geom.keys {
            if let Some(&label) = labels.get(key.key_index as usize) {
                if !label.is_empty() {
                    let font_size = if label.len() <= 2 {
                        30.0
                    } else if label.len() <= 5 {
                        20.0
                    } else {
                        16.0
                    };
                    // Soft drop shadow so legends stay razor-readable over video & ripples.
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
                        [0.95, 0.97, 1.0, 0.95],
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
        self.is_clean = false;
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let dst_a_u8 = self.rgba[idx + 3];
        if dst_a_u8 == 0 {
            self.rgba[idx] = (color[0].clamp(0.0, 1.0) * 255.0) as u8;
            self.rgba[idx + 1] = (color[1].clamp(0.0, 1.0) * 255.0) as u8;
            self.rgba[idx + 2] = (color[2].clamp(0.0, 1.0) * 255.0) as u8;
            self.rgba[idx + 3] = (a * 255.0) as u8;
            return;
        }
        let dst_a = dst_a_u8 as f32 / 255.0;
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
        let half_sw = stroke_width * 0.5;

        // Fast interior span bounds where pixels are strictly inside the rounded rect (cov=1, no stroke).
        let inset = r.max(half_sw + 2.0) + 1.0;
        let inner_x0 = ((x + inset).ceil() as i32).max(x0);
        let inner_x1 = ((x + w - inset).floor() as i32).min(x1);
        let inner_y0 = ((y + inset).ceil() as i32).max(y0);
        let inner_y1 = ((y + h - inset).floor() as i32).min(y1);

        let fill_px = [
            (color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (color[2].clamp(0.0, 1.0) * 255.0) as u8,
            (color[3].clamp(0.0, 1.0) * 255.0) as u8,
        ];

        for py in y0..y1 {
            let fy = py as f32 + 0.5;
            let dy = (fy - cy).abs() - hy;
            let in_inner_y = do_fill && py >= inner_y0 && py < inner_y1 && inner_x0 < inner_x1;
            let mut px = x0;
            while px < x1 {
                if in_inner_y && px == inner_x0 {
                    // Tight fast scanline across the flat interior (no sqrt, no stroke).
                    self.is_clean = false;
                    let row_off = (py as usize) * (self.width as usize);
                    for ix in inner_x0..inner_x1 {
                        let idx = (row_off + ix as usize) * 4;
                        if self.rgba[idx + 3] == 0 {
                            self.rgba[idx..idx + 4].copy_from_slice(&fill_px);
                        } else {
                            self.blend_px(ix, py, color, 1.0);
                        }
                    }
                    px = inner_x1;
                    continue;
                }
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
                    let sd = (d.abs() - half_sw).max(0.0);
                    let scov = (1.0 - sd).clamp(0.0, 1.0);
                    if scov > 0.0 {
                        self.blend_px(px, py, stroke_color, scov);
                    }
                }
                px += 1;
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
        if self.font.is_none() || text.is_empty() || size <= 1.0 || color[3] <= 0.001 {
            return;
        }
        let size_key = (size * 2.0).round().clamp(2.0, 400.0) as u16;
        let raster_size = size_key as f32 * 0.5;
        for ch in text.chars() {
            let key = (ch, size_key);
            if !self.glyph_cache.contains_key(&key) {
                if let Some(ref font) = self.font {
                    let entry = font.rasterize(ch, raster_size);
                    self.glyph_cache.insert(key, entry);
                }
            }
        }
        let total_w: f32 = text
            .chars()
            .filter_map(|ch| self.glyph_cache.get(&(ch, size_key)).map(|(m, _)| m.advance_width))
            .sum();
        let start_x = match align {
            TextAlign::Left => x,
            TextAlign::Center => x - total_w * 0.5,
            TextAlign::Right => x - total_w,
        };
        let baseline_y = y + size * 0.82;
        let mut pen_x = start_x;

        for ch in text.chars() {
            let Some((m, bitmap)) = self.glyph_cache.get(&(ch, size_key)).cloned() else {
                continue;
            };
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
        let mtime = std::fs::metadata(Path::new(path))
            .and_then(|m| m.modified())
            .ok();
        let needs_load = match self.image_cache.get(path) {
            None => true,
            Some((cached_m, _)) => *cached_m != mtime,
        };
        if needs_load {
            if let Ok(img) = image::open(Path::new(path)) {
                self.image_cache
                    .insert(path.to_string(), (mtime, img.to_rgba8()));
            } else if !self.image_cache.contains_key(path) {
                return;
            }
        }
        let Some((_, img)) = self.image_cache.get(path).cloned() else {
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
