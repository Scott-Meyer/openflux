use fontdue::{Font, FontSettings, Metrics};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const IMAGE_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
const GLYPH_CACHE_MAX_ENTRIES: usize = 4096;

#[derive(Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone)]
struct CachedGlyph {
    metrics: Metrics,
    bitmap: Vec<u8>,
}

struct CachedFont {
    key: String,
    font: Font,
    is_native_italic: bool,
    is_variable: bool,
    is_static_bold: bool,
}

pub struct AssetCache {
    fonts: Vec<CachedFont>,
    failed_font_keys: Vec<String>,
    glyphs: HashMap<(usize, char, u32), CachedGlyph>,
    images: HashMap<String, RgbaImage>,
    image_order: Vec<String>,
    image_total_bytes: usize,
    resolved_paths: HashMap<(String, String), Option<PathBuf>>,
    last_media_dir: String,
    last_asset_env: String,
}

impl Default for AssetCache {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetCache {
    pub fn new() -> Self {
        Self {
            fonts: Vec::new(),
            failed_font_keys: Vec::new(),
            glyphs: HashMap::new(),
            images: HashMap::new(),
            image_order: Vec::new(),
            image_total_bytes: 0,
            resolved_paths: HashMap::new(),
            last_media_dir: String::new(),
            last_asset_env: String::new(),
        }
    }

    pub fn invalidate(&mut self) {
        self.fonts.clear();
        self.failed_font_keys.clear();
        self.glyphs.clear();
        self.images.clear();
        self.image_order.clear();
        self.image_total_bytes = 0;
        self.resolved_paths.clear();
    }

    fn sync_roots(&mut self, media_dir: Option<&str>) {
        let md = media_dir.unwrap_or("");
        let env_dir = std::env::var("OPENFLUX_ASSET_DIR").unwrap_or_default();
        if self.last_media_dir != md || self.last_asset_env != env_dir {
            self.invalidate();
            self.last_media_dir = md.to_string();
            self.last_asset_env = env_dir;
        }
    }

    pub fn resolve_asset_path(
        media_dir: Option<&str>,
        cat: &str,
        filename: &str,
    ) -> Option<PathBuf> {
        if filename.is_empty() {
            return None;
        }
        let p = Path::new(filename);
        if p.is_absolute() && p.is_file() {
            return Some(p.to_path_buf());
        }
        let mut cands: Vec<PathBuf> = Vec::new();
        if let Ok(env_dir) = std::env::var("OPENFLUX_ASSET_DIR") {
            if !env_dir.is_empty() {
                if !cat.is_empty() {
                    cands.push(Path::new(&env_dir).join(cat).join(filename));
                }
                cands.push(Path::new(&env_dir).join(filename));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                let base = Path::new(&home).join(".local/share/openflux/assets");
                if !cat.is_empty() {
                    cands.push(base.join(cat).join(filename));
                }
                cands.push(base.join(filename));
            }
        }
        if let Some(md) = media_dir.filter(|s| !s.is_empty()) {
            let mdp = Path::new(md);
            if !cat.is_empty() {
                cands.push(mdp.join(cat).join(filename));
                cands.push(mdp.join("../assets").join(cat).join(filename));
            }
            cands.push(mdp.join(filename));
        }
        if !cat.is_empty() {
            cands.push(
                Path::new(crate::device_paths::DISPLAY_INSTALL_DIR)
                    .join(cat)
                    .join(filename),
            );
            cands.push(Path::new("assets").join(cat).join(filename));
            if cat == "fonts" {
                cands.push(Path::new("assets/bundled_fonts").join(filename));
            }
        }
        cands.push(Path::new(crate::device_paths::DISPLAY_INSTALL_DIR).join(filename));
        cands.push(Path::new("assets").join(filename));

        cands.into_iter().find(|c| c.is_file())
    }

    pub fn resolve_asset_path_cached(
        &mut self,
        media_dir: Option<&str>,
        cat: &str,
        filename: &str,
    ) -> Option<PathBuf> {
        self.sync_roots(media_dir);
        let key = (cat.to_string(), filename.to_string());
        if let Some(cached) = self.resolved_paths.get(&key) {
            return cached.clone();
        }
        let found = Self::resolve_asset_path(media_dir, cat, filename);
        self.resolved_paths.insert(key, found.clone());
        found
    }

    pub fn load_image(&mut self, media_dir: Option<&str>, path: &str) -> Option<&RgbaImage> {
        self.sync_roots(media_dir);
        if path.is_empty() {
            return None;
        }
        if !self.images.contains_key(path) {
            if path.to_ascii_lowercase().ends_with(".svg") {
                let meta = std::fs::metadata(path).ok()?;
                if meta.len() > 2 * 1024 * 1024 {
                    return None;
                }
                let txt = std::fs::read_to_string(path).ok()?;
                let img = rasterize_svg_to_rgba_image(&txt, 64)?;
                let bytes = img.rgba.len();
                while self.image_total_bytes + bytes > IMAGE_CACHE_MAX_BYTES
                    && !self.image_order.is_empty()
                {
                    let oldest = self.image_order.remove(0);
                    if let Some(removed) = self.images.remove(&oldest) {
                        self.image_total_bytes =
                            self.image_total_bytes.saturating_sub(removed.rgba.len());
                    }
                }
                self.image_total_bytes += bytes;
                self.image_order.push(path.to_string());
                self.images.insert(path.to_string(), img);
                return self.images.get(path);
            }
            let mut reader = image::ImageReader::open(path)
                .ok()?
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some((IMAGE_CACHE_MAX_BYTES as u64) * 2);
            reader.limits(limits.clone());
            let (dw, dh) = reader.into_dimensions().ok()?;
            if dw == 0 || dh == 0 || dw > 4096 || dh > 4096 {
                return None;
            }
            let est_bytes = (dw as usize).checked_mul(dh as usize)?.checked_mul(4)?;
            if est_bytes > IMAGE_CACHE_MAX_BYTES {
                return None;
            }
            // Re-open with configured limits for decode now that header bounds are verified
            let mut dec_reader = image::ImageReader::open(path)
                .ok()?
                .with_guessed_format()
                .ok()?;
            dec_reader.limits(limits);
            let img = dec_reader.decode().ok()?.to_rgba8();
            let (w, h) = img.dimensions();
            if w == 0 || h == 0 || w > 4096 || h > 4096 {
                return None;
            }
            let bytes = (w as usize) * (h as usize) * 4;
            if bytes > IMAGE_CACHE_MAX_BYTES {
                return None;
            }
            while self.image_total_bytes + bytes > IMAGE_CACHE_MAX_BYTES
                && !self.image_order.is_empty()
            {
                let oldest = self.image_order.remove(0);
                if let Some(removed) = self.images.remove(&oldest) {
                    self.image_total_bytes =
                        self.image_total_bytes.saturating_sub(removed.rgba.len());
                }
            }
            self.image_total_bytes += bytes;
            self.image_order.push(path.to_string());
            self.images.insert(
                path.to_string(),
                RgbaImage {
                    width: w,
                    height: h,
                    rgba: img.into_raw(),
                },
            );
        }
        self.images.get(path)
    }

    fn resolve_font_idx(
        &mut self,
        media_dir: Option<&str>,
        font_family: &str,
        font_weight: f64,
        italic: bool,
    ) -> Option<usize> {
        self.sync_roots(media_dir);
        if font_family.is_empty() {
            return None;
        }
        let clean: String = font_family
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let wsuf = weight_suffix(font_weight);
        let key = format!("{clean}|{wsuf}|{}", u8::from(italic));
        if let Some(pos) = self.fonts.iter().position(|f| f.key == key) {
            return Some(pos);
        }
        if self.failed_font_keys.iter().any(|k| k == &key) {
            return None;
        }

        let mut candidates: Vec<(String, bool)> = Vec::new();
        if italic {
            candidates.push((format!("{clean}-{wsuf}Italic.ttf"), true));
            candidates.push((format!("{clean}-Italic.ttf"), true));
            candidates.push((format!("{clean}-Italic-VariableFont_wght.ttf"), true));
            candidates.push((format!("{clean}-Italic-VariableFont_opsz,wght.ttf"), true));
        }
        candidates.push((format!("{clean}-{wsuf}.ttf"), false));
        candidates.push((format!("{clean}-Regular.ttf"), false));
        candidates.push((format!("{clean}-VariableFont_wght.ttf"), false));
        candidates.push((format!("{clean}-VariableFont_opsz,wght.ttf"), false));
        candidates.push((format!("{clean}-VariableFont_wdth,wght.ttf"), false));
        candidates.push((format!("{clean}-Bold.ttf"), false));
        candidates.push((format!("{clean}.ttf"), false));
        candidates.push((format!("{clean}.otf"), false));

        for (fname, nat_it) in candidates {
            if let Some(resolved) = Self::resolve_asset_path(media_dir, "fonts", &fname) {
                if let Ok(bytes) = std::fs::read(&resolved) {
                    if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                        let path_s = resolved.to_string_lossy();
                        let is_variable = path_s.contains("VariableFont");
                        let is_static_bold = path_s.contains("Bold")
                            || path_s.contains("Black")
                            || path_s.contains("Medium")
                            || path_s.contains("SemiBold");
                        let idx = self.fonts.len();
                        self.fonts.push(CachedFont {
                            key,
                            font,
                            is_native_italic: nat_it,
                            is_variable,
                            is_static_bold,
                        });
                        return Some(idx);
                    }
                }
            }
        }
        self.failed_font_keys.push(key);
        None
    }

    pub fn measure_text(
        &mut self,
        media_dir: Option<&str>,
        font_family: &str,
        font_weight: f64,
        italic: bool,
        px_height: i32,
        text: &str,
    ) -> i32 {
        let Some(fidx) = self.resolve_font_idx(media_dir, font_family, font_weight, italic) else {
            return text.chars().count() as i32 * (6 * (px_height / 7).max(1));
        };
        let px = px_height.max(6) as f32;
        let font = &self.fonts[fidx].font;
        let mut w = 0.0_f32;
        for ch in text.chars() {
            w += font.metrics(ch, px).advance_width;
        }
        w.ceil() as i32
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        media_dir: Option<&str>,
        font_family: &str,
        font_weight: f64,
        italic: bool,
        pixels: &mut [u32],
        buf_w: i32,
        buf_h: i32,
        x: i32,
        y: i32,
        px_height: i32,
        argb: u32,
        text: &str,
    ) -> i32 {
        let Some(fidx) = self.resolve_font_idx(media_dir, font_family, font_weight, italic) else {
            return draw_bitmap_fallback(pixels, buf_w, buf_h, x, y, px_height, argb, text);
        };
        let px_u32 = px_height.max(6) as u32;
        let px_f = px_u32 as f32;
        let (is_nat_italic, is_var, is_bold, ascent_px) = {
            let f = &self.fonts[fidx];
            let asc = f
                .font
                .horizontal_line_metrics(px_f)
                .map(|m| m.ascent)
                .unwrap_or(px_f * 0.8);
            (f.is_native_italic, f.is_variable, f.is_static_bold, asc)
        };
        let shear = if italic && !is_nat_italic {
            0.22_f32
        } else {
            0.0_f32
        };
        // Intentional 1px double-strike on non-bold faces so small keycap legends stay crisp at 1080p.
        let bold_passes = if !is_bold && (font_weight >= 350.0 || (is_var && font_weight >= 180.0))
        {
            2
        } else {
            1
        };

        let baseline_y = y as f32 + ascent_px;
        let mut cursor_x = x as f32;

        for ch in text.chars() {
            if self.glyphs.len() > GLYPH_CACHE_MAX_ENTRIES {
                self.glyphs.clear();
            }
            let key = (fidx, ch, px_u32);
            if !self.glyphs.contains_key(&key) {
                let (metrics, bitmap) = self.fonts[fidx].font.rasterize(ch, px_f);
                self.glyphs.insert(key, CachedGlyph { metrics, bitmap });
            }
            let glyph = &self.glyphs[&key];
            let gw = glyph.metrics.width as i32;
            let gh = glyph.metrics.height as i32;
            let gx0 = cursor_x.round() as i32 + glyph.metrics.xmin;
            let gy0 = baseline_y.round() as i32 - glyph.metrics.ymin - gh;

            if gw > 0 && gh > 0 {
                for row in 0..gh {
                    let py = gy0 + row;
                    if py < 0 || py >= buf_h {
                        continue;
                    }
                    let it_dx = if shear > 0.0 {
                        -(((gy0 + row) as f32 - baseline_y) * shear).round() as i32
                    } else {
                        0
                    };
                    let row_off = (row * gw) as usize;
                    let pix_row = (py * buf_w) as usize;
                    for col in 0..gw {
                        let mut cov = glyph.bitmap[row_off + col as usize];
                        if cov == 0 {
                            continue;
                        }
                        if is_var && cov < 255 {
                            let c = cov as u32 + ((255 - cov as u32) * 38) / 100;
                            cov = c.min(255) as u8;
                        }
                        for bp in 0..bold_passes {
                            let px = gx0 + col + it_dx + bp;
                            if px >= 0 && px < buf_w {
                                blend_argb_cov(&mut pixels[pix_row + px as usize], argb, cov);
                            }
                        }
                    }
                }
            }
            cursor_x += glyph.metrics.advance_width;
        }
        cursor_x.ceil() as i32 - x
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_weather_svg(
        &mut self,
        media_dir: Option<&str>,
        condition: &str,
        pixels: &mut [u32],
        buf_w: i32,
        buf_h: i32,
        dx: i32,
        dy: i32,
        size_px: i32,
        argb: u32,
    ) -> bool {
        let is_day = !condition.to_ascii_lowercase().contains("night");
        self.draw_weather_svg_with_day(
            media_dir, condition, is_day, pixels, buf_w, buf_h, dx, dy, size_px, argb,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_weather_svg_with_day(
        &mut self,
        media_dir: Option<&str>,
        condition: &str,
        is_day: bool,
        pixels: &mut [u32],
        buf_w: i32,
        buf_h: i32,
        dx: i32,
        dy: i32,
        size_px: i32,
        argb: u32,
    ) -> bool {
        self.sync_roots(media_dir);
        let lc = condition.to_ascii_lowercase();
        let night = !is_day || lc.contains("night");
        let fname = if lc.contains("thunder") || lc.contains("storm") {
            "wi-thunderstorm.svg"
        } else if lc.contains("hail") {
            "wi-hail.svg"
        } else if lc.contains("sleet") {
            "wi-sleet.svg"
        } else if lc.contains("snow") || lc.contains("flurr") {
            "wi-snow.svg"
        } else if lc.contains("fog") || lc.contains("mist") || lc.contains("haze") {
            "wi-fog.svg"
        } else if lc.contains("shower") {
            "wi-showers.svg"
        } else if lc.contains("rain") || lc.contains("drizzle") {
            if night {
                "wi-night-alt-rain-mix.svg"
            } else {
                "wi-rain.svg"
            }
        } else if lc.contains("partly") || lc.contains("few") {
            if night {
                "wi-night-alt-cloudy.svg"
            } else {
                "wi-day-cloudy.svg"
            }
        } else if lc.contains("cloud") || lc.contains("overcast") {
            if night {
                "wi-night-alt-cloudy.svg"
            } else {
                "wi-cloudy.svg"
            }
        } else if lc.contains("sun") || lc.contains("clear") {
            if night {
                "wi-night-clear.svg"
            } else {
                "wi-day-sunny.svg"
            }
        } else if night {
            "wi-night-alt-cloudy.svg"
        } else {
            "wi-day-cloudy.svg"
        };

        let Some(path) = Self::resolve_asset_path(media_dir, "weather_icons", fname) else {
            return false;
        };
        let key = format!("wx:{}:{size_px}", path.to_string_lossy());
        if !self.images.contains_key(&key) {
            let Ok(txt) = std::fs::read_to_string(&path) else {
                return false;
            };
            let Some(img) = rasterize_svg_to_rgba_image(&txt, (size_px.max(16)) as u32) else {
                return false;
            };
            self.cache_image(key.clone(), img);
        }
        self.blit_cached_tinted_svg(&key, pixels, buf_w, buf_h, dx, dy, argb)
    }

    /// Standard Lucide (ISC) 24x24 vector icons rendered crisply via resvg (`stroke-width=2`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_lucide_shortcut_svg(
        &mut self,
        label: &str,
        pixels: &mut [u32],
        buf_w: i32,
        buf_h: i32,
        cx: i32,
        cy: i32,
        size_px: i32,
        argb: u32,
    ) -> bool {
        let has_word = |hay: &str, w: &str| {
            hay.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|tok| tok == w)
        };
        let l = label.to_ascii_lowercase();
        let (name, body) = if l.contains("copy") || l.contains("dup") {
            (
                "copy",
                r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#,
            )
        } else if l.contains("paste") {
            (
                "paste",
                r#"<path d="M15 2H9a1 1 0 0 0-1 1v2c0 .6.4 1 1 1h6c.6 0 1-.4 1-1V3c0-.6-.4-1-1-1Z"/><path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2M16 4h2a2 2 0 0 1 2 2v2M11 14h10"/><path d="m17 10 4 4-4 4"/>"#,
            )
        } else if has_word(&l, "cut") {
            (
                "cut",
                r#"<circle cx="6" cy="6" r="3"/><path d="M8.12 8.12 12 12"/><path d="M20 4 8.12 15.88"/><circle cx="6" cy="18" r="3"/><path d="M14.8 14.8 20 20"/>"#,
            )
        } else if l.contains("undo") {
            (
                "undo",
                r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/>"#,
            )
        } else if l.contains("redo") {
            (
                "redo",
                r#"<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13"/>"#,
            )
        } else if l.contains("save") {
            (
                "save",
                r#"<path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7"/><path d="M7 3v4a1 1 0 0 0 1 1h7"/>"#,
            )
        } else if l.contains("palette") {
            (
                "palette",
                r#"<circle cx="13.5" cy="6.5" r=".5" fill="white"/><circle cx="17.5" cy="10.5" r=".5" fill="white"/><circle cx="8.5" cy="7.5" r=".5" fill="white"/><circle cx="6.5" cy="12.5" r=".5" fill="white"/><path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"/>"#,
            )
        } else if l.contains("find") || l.contains("search") {
            (
                "search",
                r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#,
            )
        } else if l == "all" || l.contains("select") {
            (
                "select",
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="m9 12 2 2 4-4"/>"#,
            )
        } else if l.contains("close") {
            ("close", r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#)
        } else if l == "new" || l.contains("add") {
            ("plus", r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#)
        } else if l.contains("tab") {
            (
                "tab",
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/><path d="M9 3v6"/>"#,
            )
        } else if l.contains("quit") || l.contains("power") {
            (
                "power",
                r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#,
            )
        } else if l.contains("reload") || l.contains("refresh") {
            (
                "refresh",
                r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#,
            )
        } else if l.contains("delete") || l.contains("trash") || l.contains("remove") {
            (
                "trash",
                r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#,
            )
        } else if l.contains("zoom in") {
            (
                "zoomin",
                r#"<circle cx="11" cy="11" r="8"/><line x1="21" x2="16.65" y1="21" y2="16.65"/><line x1="11" x2="11" y1="8" y2="14"/><line x1="8" x2="14" y1="11" y2="11"/>"#,
            )
        } else if l.contains("zoom out") {
            (
                "zoomout",
                r#"<circle cx="11" cy="11" r="8"/><line x1="21" x2="16.65" y1="21" y2="16.65"/><line x1="8" x2="14" y1="11" y2="11"/>"#,
            )
        } else if l.contains("terminal") || l.contains("console") {
            (
                "terminal",
                r#"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"#,
            )
        } else if has_word(&l, "open") || l.contains("folder") {
            (
                "folder",
                r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#,
            )
        } else if l.contains("setting") || l.contains("pref") {
            (
                "settings",
                r#"<line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/><line x1="14" x2="14" y1="2" y2="6"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="16" x2="16" y1="18" y2="22"/>"#,
            )
        } else if l.contains("split") {
            (
                "split",
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 3v18"/>"#,
            )
        } else if l.contains("comment") {
            (
                "comment",
                r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#,
            )
        } else if l.contains("lock") {
            (
                "lock",
                r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#,
            )
        } else if l.contains("print") {
            (
                "print",
                r#"<path d="M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2"/><path d="M6 9V3a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v6"/><rect x="6" y="14" width="12" height="8" rx="1"/>"#,
            )
        } else if l.contains("play") {
            ("play", r#"<polygon points="6 3 20 12 6 21 6 3"/>"#)
        } else if l.contains("pause") {
            (
                "pause",
                r#"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"#,
            )
        } else {
            return false;
        };
        let sz = size_px.max(16);
        let key = format!("lucide:{name}:{sz}");
        if !self.images.contains_key(&key) {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
            );
            let Some(img) = rasterize_svg_to_rgba_image(&svg, sz as u32) else {
                return false;
            };
            self.cache_image(key.clone(), img);
        }
        self.blit_cached_tinted_svg(&key, pixels, buf_w, buf_h, cx - sz / 2, cy - sz / 2, argb)
    }

    /// Standard Lucide (ISC) 24x24 vector icons for FN-layer media, brightness, and OpenFlux function keycodes.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_lucide_keycode_svg(
        &mut self,
        key_code: &str,
        pixels: &mut [u32],
        buf_w: i32,
        buf_h: i32,
        cx: i32,
        cy: i32,
        size_px: i32,
        argb: u32,
    ) -> bool {
        let (name, body) = match key_code {
            "KC_AUDIO_VOL_UP" => (
                "vol-up",
                r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>"#,
            ),
            "KC_AUDIO_VOL_DOWN" => (
                "vol-down",
                r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/>"#,
            ),
            "KC_AUDIO_MUTE" => (
                "vol-mute",
                r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><line x1="22" x2="16" y1="9" y2="15"/><line x1="16" x2="22" y1="9" y2="15"/>"#,
            ),
            "KC_MEDIA_PLAY_PAUSE" => (
                "play-pause",
                r#"<polygon points="4 5 13 12 4 19 4 5"/><line x1="17" x2="17" y1="5" y2="19"/><line x1="21" x2="21" y1="5" y2="19"/>"#,
            ),
            "KC_MEDIA_NEXT_TRACK" => (
                "skip-fwd",
                r#"<polygon points="5 4 15 12 5 20 5 4"/><line x1="19" x2="19" y1="5" y2="19"/>"#,
            ),
            "KC_MEDIA_PREV_TRACK" => (
                "skip-back",
                r#"<polygon points="19 20 9 12 19 4 19 20"/><line x1="5" x2="5" y1="19" y2="5"/>"#,
            ),
            "KC_MEDIA_STOP" => (
                "stop",
                r#"<rect width="14" height="14" x="5" y="5" rx="2"/>"#,
            ),
            "KC_BRIGHTNESS_UP" | "KC_DISPLAY_BRIGHTNESS_UP" | "KC_OF_BRIGHTNESS_UP" => (
                "bri-up",
                r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#,
            ),
            "KC_BRIGHTNESS_DOWN" | "KC_DISPLAY_BRIGHTNESS_DOWN" | "KC_OF_BRIGHTNESS_DOWN" => (
                "bri-down",
                r#"<circle cx="12" cy="12" r="4"/><path d="M12 4v1"/><path d="M12 19v1"/><path d="M4 12h1"/><path d="M19 12h1"/><path d="m6.3 6.3.7.7"/><path d="m17 17 .7.7"/><path d="m6.3 17.7.7-.7"/><path d="m17 7 .7-.7"/>"#,
            ),
            "KC_OF_START_PAUSE_POMODORO" => (
                "pomo",
                r#"<line x1="10" x2="14" y1="2" y2="2"/><line x1="12" x2="15" y1="14" y2="11"/><circle cx="12" cy="14" r="8"/>"#,
            ),
            "KC_OF_RESET_POMODORO" => (
                "pomo-rst",
                r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#,
            ),
            "KC_OF_PROFILE_CYCLE_NEXT" | "KC_OF_PROFILE_CYCLE_PREV" => (
                "layers",
                r#"<path d="m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z"/><path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65"/><path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65"/>"#,
            ),
            "KC_OF_THEME_CYCLE_NEXT" | "KC_OF_THEME_CYCLE_PREV" => (
                "palette",
                r#"<circle cx="13.5" cy="6.5" r=".5" fill="white"/><circle cx="17.5" cy="10.5" r=".5" fill="white"/><circle cx="8.5" cy="7.5" r=".5" fill="white"/><circle cx="6.5" cy="12.5" r=".5" fill="white"/><path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"/>"#,
            ),
            "KC_OF_ACTUATION_MODE_NEXT" | "KC_OF_ACTUATION_MODE_PREV" => (
                "sliders",
                r#"<line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/>"#,
            ),
            "KC_MISSION_CONTROL" => (
                "mission",
                r#"<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>"#,
            ),
            "KC_LAUNCHPAD" => (
                "launchpad",
                r#"<circle cx="6" cy="6" r="1.5" fill="white"/><circle cx="12" cy="6" r="1.5" fill="white"/><circle cx="18" cy="6" r="1.5" fill="white"/><circle cx="6" cy="12" r="1.5" fill="white"/><circle cx="12" cy="12" r="1.5" fill="white"/><circle cx="18" cy="12" r="1.5" fill="white"/><circle cx="6" cy="18" r="1.5" fill="white"/><circle cx="12" cy="18" r="1.5" fill="white"/><circle cx="18" cy="18" r="1.5" fill="white"/>"#,
            ),
            "KC_WWW_SEARCH" | "KC_SEARCH" => (
                "kc-search",
                r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#,
            ),
            _ => return false,
        };
        let sz = size_px.max(16);
        let key = format!("lucide-kc:{name}:{sz}");
        if !self.images.contains_key(&key) {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
            );
            let Some(img) = rasterize_svg_to_rgba_image(&svg, sz as u32) else {
                return false;
            };
            self.cache_image(key.clone(), img);
        }
        self.blit_cached_tinted_svg(&key, pixels, buf_w, buf_h, cx - sz / 2, cy - sz / 2, argb)
    }

    fn cache_image(&mut self, key: String, img: RgbaImage) -> bool {
        let bytes = img.rgba.len();
        if bytes > IMAGE_CACHE_MAX_BYTES {
            return false;
        }
        while self.image_total_bytes + bytes > IMAGE_CACHE_MAX_BYTES && !self.image_order.is_empty()
        {
            let oldest = self.image_order.remove(0);
            if let Some(removed) = self.images.remove(&oldest) {
                self.image_total_bytes = self.image_total_bytes.saturating_sub(removed.rgba.len());
            }
        }
        self.image_total_bytes += bytes;
        self.image_order.push(key.clone());
        self.images.insert(key, img);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn blit_cached_tinted_svg(
        &self,
        key: &str,
        pixels: &mut [u32],
        buf_w: i32,
        buf_h: i32,
        dx: i32,
        dy: i32,
        argb: u32,
    ) -> bool {
        let Some(im) = self.images.get(key) else {
            return false;
        };
        let iw = im.width as i32;
        let ih = im.height as i32;
        for py in 0..ih {
            let oy = dy + py;
            if oy < 0 || oy >= buf_h {
                continue;
            }
            let row = (oy * buf_w) as usize;
            let srow = (py * iw * 4) as usize;
            for px in 0..iw {
                let ox = dx + px;
                if ox < 0 || ox >= buf_w {
                    continue;
                }
                let a = im.rgba[srow + (px as usize) * 4 + 3];
                if a > 0 {
                    blend_argb_cov(&mut pixels[row + ox as usize], argb, a);
                }
            }
        }
        true
    }
}

fn weight_suffix(w: f64) -> &'static str {
    if w <= 150.0 {
        "Thin"
    } else if w <= 250.0 {
        "ExtraLight"
    } else if w <= 350.0 {
        "Light"
    } else if w <= 450.0 {
        "Regular"
    } else if w <= 550.0 {
        "Medium"
    } else if w <= 650.0 {
        "SemiBold"
    } else if w <= 750.0 {
        "Bold"
    } else if w <= 850.0 {
        "ExtraBold"
    } else {
        "Black"
    }
}

#[inline]
pub fn blend_argb_cov(dst: &mut u32, src_argb: u32, cov: u8) {
    let raw_sa = ((src_argb >> 24) & 0xFF) * cov as u32;
    let sa = (raw_sa + 1 + (raw_sa >> 8)) >> 8;
    if sa == 0 {
        return;
    }
    if sa >= 255 {
        *dst = 0xFF00_0000 | (src_argb & 0x00FF_FFFF);
        return;
    }
    let sr = (src_argb >> 16) & 0xFF;
    let sg = (src_argb >> 8) & 0xFF;
    let sb = src_argb & 0xFF;
    let d = *dst;
    let da = (d >> 24) & 0xFF;
    let dr = (d >> 16) & 0xFF;
    let dg = (d >> 8) & 0xFF;
    let db = d & 0xFF;
    let inv = 255 - sa;
    let mut t = sr * sa + dr * inv;
    let r = (t + 1 + (t >> 8)) >> 8;
    t = sg * sa + dg * inv;
    let g = (t + 1 + (t >> 8)) >> 8;
    t = sb * sa + db * inv;
    let b = (t + 1 + (t >> 8)) >> 8;
    let a = if da >= 255 {
        255
    } else {
        let ta = da * inv;
        (sa + ((ta + 1 + (ta >> 8)) >> 8)).min(255)
    };
    *dst = (a << 24) | (r << 16) | (g << 8) | b;
}

fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x04, 0x04, 0x00, 0x04, 0x04, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '\\' => [0x10, 0x08, 0x08, 0x04, 0x02, 0x02, 0x01],
        '[' => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
        ']' => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
        ';' => [0x00, 0x04, 0x04, 0x00, 0x04, 0x04, 0x08],
        '\'' => [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],
        '`' => [0x08, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '<' => [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '^' => [0x04, 0x0E, 0x15, 0x04, 0x04, 0x04, 0x04],
        '|' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        '#' => [0x0A, 0x1F, 0x0A, 0x0A, 0x1F, 0x0A, 0x00],
        '°' => [0x0C, 0x12, 0x12, 0x0C, 0x00, 0x00, 0x00],
        ' ' => [0; 7],
        _ => [0x0E, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_bitmap_fallback(
    pixels: &mut [u32],
    buf_w: i32,
    buf_h: i32,
    x: i32,
    y: i32,
    px_height: i32,
    argb: u32,
    text: &str,
) -> i32 {
    let scale = (px_height / 7).max(1);
    let mut cx = x;
    for ch in text.chars() {
        let rows = glyph_5x7(ch);
        for (ry, bits) in rows.iter().enumerate() {
            for rx in 0..5 {
                if ((bits >> (4 - rx)) & 1) != 0 {
                    for sy in 0..scale {
                        let py = y + ry as i32 * scale + sy;
                        if py < 0 || py >= buf_h {
                            continue;
                        }
                        for sx in 0..scale {
                            let px = cx + rx * scale + sx;
                            if px >= 0 && px < buf_w {
                                blend_argb_cov(&mut pixels[(py * buf_w + px) as usize], argb, 255);
                            }
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
    cx - x
}

fn rasterize_svg_to_rgba_image(svg_txt: &str, out_sz: u32) -> Option<RgbaImage> {
    if out_sz == 0 {
        return None;
    }
    let est_bytes = (out_sz as usize)
        .checked_mul(out_sz as usize)?
        .checked_mul(4)?;
    if est_bytes > IMAGE_CACHE_MAX_BYTES {
        return None;
    }
    let mut opt = resvg::usvg::Options {
        resources_dir: None,
        ..Default::default()
    };
    opt.image_href_resolver.resolve_string = Box::new(|_, _| None);
    opt.image_href_resolver.resolve_data = Box::new(|_, _, _| None);
    let tree = resvg::usvg::Tree::from_str(svg_txt, &opt).ok()?;
    let sz = tree.size();
    if sz.width() <= 0.0 || sz.height() <= 0.0 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(out_sz, out_sz)?;
    let scale = (out_sz as f32 / sz.width()).min(out_sz as f32 / sz.height());
    let tx = (out_sz as f32 - sz.width() * scale) * 0.5;
    let ty = (out_sz as f32 - sz.height() * scale) * 0.5;
    let ts = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    resvg::render(&tree, ts, &mut pixmap.as_mut());
    // Convert tiny-skia premultiplied RGBA into straight RGBA so alpha sprite blending is clean.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            px[0] = ((px[0] as u16 * 255) / a as u16).min(255) as u8;
            px[1] = ((px[1] as u16 * 255) / a as u16).min(255) as u8;
            px[2] = ((px[2] as u16 * 255) / a as u16).min(255) as u8;
        }
    }
    Some(RgbaImage {
        width: out_sz,
        height: out_sz,
        rgba,
    })
}
