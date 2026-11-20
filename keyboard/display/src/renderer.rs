use crate::assets::{blend_argb_cov, AssetCache};
use crate::config::{
    DisplayConfig, KeyEffect, LiveState, LockIndicatorStyle, MosaicTileConfig, TileType,
    HW_HIST_LEN,
};
use crate::layout::{
    build_physical_layout, compute_key_parallax, resolve_key_style, FrameLayoutType, PhysicalLayout,
};
use crate::media_bg::BgCache;

pub struct Renderer {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
    pub bg: BgCache,
    pub assets: AssetCache,
    pub phys_layout: PhysicalLayout,
    pub cached_layout_type: FrameLayoutType,
}

impl Renderer {
    pub fn new(width: i32, height: i32) -> Self {
        let w = width.max(1);
        let h = height.max(1);
        Self {
            width: w,
            height: h,
            pixels: vec![0xFF00_0000; (w as usize) * (h as usize)],
            bg: BgCache::new(w, h),
            assets: AssetCache::new(),
            phys_layout: build_physical_layout(FrameLayoutType::Ansi),
            cached_layout_type: FrameLayoutType::Ansi,
        }
    }

    pub fn render_frame_at(
        &mut self,
        cfg: &DisplayConfig,
        st: &LiveState,
        media_dir: Option<&str>,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
    ) {
        self.render_internal(cfg, st, media_dir, now_sec, now_ms, suspended, false);
    }

    /// Renders only the transparent-background UI (key legends, icons, Mosaic widgets, and dividers)
    /// into `self.pixels` as premultiplied ARGB for GPU compositing (`wayland_egl`).
    pub fn render_ui_overlay_at(
        &mut self,
        cfg: &DisplayConfig,
        st: &LiveState,
        media_dir: Option<&str>,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
    ) {
        self.render_internal(cfg, st, media_dir, now_sec, now_ms, suspended, true);
    }

    pub fn collect_gpu_glass_rects(
        &mut self,
        cfg: &DisplayConfig,
    ) -> Vec<(f32, f32, f32, f32, f32, u32)> {
        if self.cached_layout_type != cfg.keymap.selected_layout {
            self.phys_layout = build_physical_layout(cfg.keymap.selected_layout);
            self.cached_layout_type = cfg.keymap.selected_layout;
        }
        let mut out = Vec::new();
        let has_any_tile = cfg
            .mosaic
            .tiles
            .iter()
            .any(|t| t.tile_type != TileType::None);
        if has_any_tile && cfg.appearance.module_dimming > 1.0 {
            let dim_a = ((cfg.appearance.module_dimming / 100.0) * 140.0).clamp(0.0, 180.0) as u32;
            out.push((
                self.phys_layout.mosaic_x,
                self.phys_layout.mosaic_y,
                self.phys_layout.mosaic_w,
                self.phys_layout.mosaic_h,
                14.0,
                (dim_a << 24) | 0x0007_090E,
            ));
        }
        out
    }

    #[allow(clippy::type_complexity)]
    pub fn collect_gpu_key_effects(
        &self,
        cfg: &DisplayConfig,
        st: &LiveState,
        now_ms: u64,
    ) -> Vec<(f32, f32, f32, f32, i32, f32, f32, f32, u32)> {
        let mut out = Vec::new();
        let kind = match cfg.appearance.key_effect {
            KeyEffect::GlowingRings => 1,
            KeyEffect::SolidHighlight => 2,
            KeyEffect::Ripple => 3,
            KeyEffect::AutofocusLock => 4,
            KeyEffect::None => return out,
        };
        let eff_layer = st.effective_layer(&cfg.keymap);
        let fallback_layer =
            (cfg.keymap.default_layer as usize).min(cfg.keymap.layers.len().saturating_sub(1));
        let style_layer = cfg.appearance.stylemaps.get(&eff_layer);
        let actions = cfg
            .keymap
            .layers
            .get(eff_layer)
            .map(|l| l.actions.as_slice())
            .unwrap_or(&[]);
        let fallback_actions = cfg
            .keymap
            .layers
            .get(fallback_layer)
            .map(|l| l.actions.as_slice())
            .unwrap_or(&[]);

        if cfg.appearance.key_effect == KeyEffect::Ripple {
            let mut drew_any_pulse = false;
            for &(kidx, start_ms) in &st.ripple_pulses {
                if now_ms < start_ms || (now_ms - start_ms) >= 2100 {
                    continue;
                }
                let Some(kr) = self
                    .phys_layout
                    .keys
                    .iter()
                    .find(|r| r.key_index as usize == kidx)
                else {
                    continue;
                };
                let (_, _, _, ring_col) = resolve_key_style(
                    actions,
                    fallback_actions,
                    style_layer,
                    cfg.appearance.legend_argb,
                    cfg.appearance.ring_glow_argb,
                    kidx,
                );
                let (dx, dy) = compute_key_parallax(
                    &cfg.settings,
                    kr.x as f64 + kr.w as f64 * 0.5,
                    kr.y as f64 + kr.h as f64 * 0.5,
                );
                let kcx = kr.x + dx as f32 + kr.w * 0.5;
                let kcy = kr.y + dy as f32 + kr.h * 0.5;
                let age_ms = (now_ms - start_ms) as f32;
                out.push((kcx, kcy, kr.w, kr.h, kind, age_ms, 0.0, 1.0, ring_col));
                drew_any_pulse = true;
            }
            if drew_any_pulse {
                return out;
            }
        }

        for kr in &self.phys_layout.keys {
            let kidx = kr.key_index as usize;
            let pressed = st.is_key_pressed(kidx);
            let rel_ms = if kidx < st.key_release_ms.len() {
                st.key_release_ms[kidx]
            } else {
                0
            };
            let press_ms = if kidx < st.key_press_ms.len() {
                st.key_press_ms[kidx]
            } else {
                0
            };
            let glow = if pressed {
                1.0_f32
            } else if rel_ms > 0 && now_ms >= rel_ms && (now_ms - rel_ms) < 480 {
                let lin = 1.0_f32 - (now_ms - rel_ms) as f32 / 480.0;
                lin * lin
            } else {
                0.0
            };
            if glow <= 0.01 {
                continue;
            }
            let (_, _, _, ring_col) = resolve_key_style(
                actions,
                fallback_actions,
                style_layer,
                cfg.appearance.legend_argb,
                cfg.appearance.ring_glow_argb,
                kidx,
            );
            let (dx, dy) = compute_key_parallax(
                &cfg.settings,
                kr.x as f64 + kr.w as f64 * 0.5,
                kr.y as f64 + kr.h as f64 * 0.5,
            );
            let kcx = kr.x + dx as f32 + kr.w * 0.5;
            let kcy = kr.y + dy as f32 + kr.h * 0.5;
            let age_ms = if press_ms > 0 && now_ms >= press_ms {
                (now_ms - press_ms) as f32
            } else {
                280.0
            };
            let rel_age = if !pressed && rel_ms > 0 && now_ms >= rel_ms {
                (now_ms - rel_ms) as f32
            } else {
                0.0
            };
            out.push((kcx, kcy, kr.w, kr.h, kind, age_ms, rel_age, glow, ring_col));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal(
        &mut self,
        cfg: &DisplayConfig,
        st: &LiveState,
        media_dir: Option<&str>,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
        overlay_only: bool,
    ) {
        if suspended {
            self.pixels.fill(if overlay_only {
                0x0000_0000
            } else {
                0xFF00_0000
            });
            return;
        }
        if self.cached_layout_type != cfg.keymap.selected_layout {
            self.phys_layout = build_physical_layout(cfg.keymap.selected_layout);
            self.cached_layout_type = cfg.keymap.selected_layout;
        }

        if overlay_only {
            self.pixels.fill(0x0000_0000);
        } else {
            self.bg
                .step(&mut self.assets, &cfg.appearance, media_dir, now_ms);
            let copy_len = self.pixels.len().min(self.bg.pixels.len());
            self.pixels[..copy_len].copy_from_slice(&self.bg.pixels[..copy_len]);
        }

        // 1. Key press effects and alignment boxes first, matching GPU shader order so cross-key ripples do not paint over Mosaic text or neighboring legends.
        let eff_layer = st.effective_layer(&cfg.keymap);
        let fallback_layer =
            (cfg.keymap.default_layer as usize).min(cfg.keymap.layers.len().saturating_sub(1));
        let keys = self.phys_layout.keys.clone();
        let style_layer = cfg.appearance.stylemaps.get(&eff_layer).cloned();
        let actions = cfg
            .keymap
            .layers
            .get(eff_layer)
            .map(|l| l.actions.as_slice())
            .unwrap_or(&[]);
        let fallback_actions = cfg
            .keymap
            .layers
            .get(fallback_layer)
            .map(|l| l.actions.as_slice())
            .unwrap_or(&[]);

        for kr in &keys {
            let (dx, dy) = compute_key_parallax(
                &cfg.settings,
                kr.x as f64 + kr.w as f64 * 0.5,
                kr.y as f64 + kr.h as f64 * 0.5,
            );
            let kx = (kr.x + dx as f32) as i32;
            let ky = (kr.y + dy as f32) as i32;
            let kw = kr.w as i32;
            let kh = kr.h as i32;
            let kidx = kr.key_index as usize;
            let pressed = st.is_key_pressed(kidx);
            let (_, _, _, ring_col) = resolve_key_style(
                actions,
                fallback_actions,
                style_layer.as_ref(),
                cfg.appearance.legend_argb,
                cfg.appearance.ring_glow_argb,
                kidx,
            );
            let rel_ms = if kidx < st.key_release_ms.len() {
                st.key_release_ms[kidx]
            } else {
                0
            };
            let press_ms = if kidx < st.key_press_ms.len() {
                st.key_press_ms[kidx]
            } else {
                0
            };
            let glow = if pressed {
                1.0_f32
            } else if rel_ms > 0 && now_ms >= rel_ms && (now_ms - rel_ms) < 480 {
                let lin = 1.0_f32 - (now_ms - rel_ms) as f32 / 480.0;
                lin * lin
            } else {
                0.0
            };

            if cfg.settings.show_alignment_grid {
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    kx,
                    ky,
                    kw,
                    kh,
                    9,
                    0x3808_0B12,
                );
                draw_round_rect_outline(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    kx,
                    ky,
                    kw,
                    kh,
                    9,
                    1,
                    0x66FF_FFFF,
                );
            }

            let kcx = kx as f32 + kw as f32 * 0.5;
            let kcy = ky as f32 + kh as f32 * 0.5;

            if !overlay_only && glow > 0.01 {
                match cfg.appearance.key_effect {
                    KeyEffect::GlowingRings => {
                        let base_r = (kw.min(kh) as f32) * 0.34;
                        let a_core = (glow * 235.0) as u32;
                        let a_mid = (glow * 120.0) as u32;
                        let a_halo = (glow * 55.0) as u32;
                        draw_ring(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kcx,
                            kcy,
                            base_r + 6.5,
                            5.0,
                            (a_halo << 24) | (ring_col & 0x00FF_FFFF),
                        );
                        draw_ring(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kcx,
                            kcy,
                            base_r + 3.0,
                            3.5,
                            (a_mid << 24) | (ring_col & 0x00FF_FFFF),
                        );
                        draw_ring(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kcx,
                            kcy,
                            base_r,
                            4.2,
                            (a_core << 24) | (ring_col & 0x00FF_FFFF),
                        );
                        if press_ms > 0 && now_ms >= press_ms && (now_ms - press_ms) < 420 {
                            let pt = (now_ms - press_ms) as f32 / 420.0;
                            let pr = base_r + pt * 18.0;
                            let pa = ((1.0 - pt) * 165.0) as u32;
                            draw_ring(
                                &mut self.pixels,
                                self.width,
                                self.height,
                                kcx,
                                kcy,
                                pr,
                                2.2,
                                (pa << 24) | (ring_col & 0x00FF_FFFF),
                            );
                        }
                    }
                    KeyEffect::SolidHighlight => {
                        let bloom_a = (glow * 185.0) as u32;
                        fill_round_rect_alpha(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kx + 1,
                            ky + 1,
                            kw - 2,
                            kh - 2,
                            8,
                            (bloom_a << 24) | (ring_col & 0x00FF_FFFF),
                        );
                    }
                    KeyEffect::Ripple => {}
                    KeyEffect::AutofocusLock => {
                        let age_ms = if press_ms > 0 && now_ms >= press_ms {
                            now_ms - press_ms
                        } else {
                            280
                        };
                        let rel_age = if !pressed && rel_ms > 0 && now_ms >= rel_ms {
                            (now_ms - rel_ms) as f32
                        } else {
                            0.0
                        };
                        draw_autofocus_lock(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kcx,
                            kcy,
                            kw as f32,
                            kh as f32,
                            age_ms as f32,
                            rel_age,
                            glow,
                            ring_col,
                        );
                    }
                    KeyEffect::None => {}
                }
            }
        }

        // 1b. Ripple pass: one expanding outline ring per recorded press over 2100 ms, without solid fill.
        if !overlay_only && cfg.appearance.key_effect == KeyEffect::Ripple {
            for &(pk, start_ms) in &st.ripple_pulses {
                if now_ms < start_ms || (now_ms - start_ms) >= 2100 {
                    continue;
                }
                let Some(kr) = keys.iter().find(|r| r.key_index as usize == pk) else {
                    continue;
                };
                let (dx, dy) = compute_key_parallax(
                    &cfg.settings,
                    kr.x as f64 + kr.w as f64 * 0.5,
                    kr.y as f64 + kr.h as f64 * 0.5,
                );
                let kx = (kr.x + dx as f32) as i32;
                let ky = (kr.y + dy as f32) as i32;
                let kw = kr.w as i32;
                let kh = kr.h as i32;
                let kcx = kx as f32 + kw as f32 * 0.5;
                let kcy = ky as f32 + kh as f32 * 0.5;
                let (_, _, _, ring_col) = resolve_key_style(
                    actions,
                    fallback_actions,
                    style_layer.as_ref(),
                    cfg.appearance.legend_argb,
                    cfg.appearance.ring_glow_argb,
                    pk,
                );
                let base_r = (kw.min(kh) as f32) * 0.34;
                let unit = kw.min(kh) as f32;
                let age_ms = (now_ms - start_ms) as f32;
                let pt = (age_ms / 2100.0).clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - pt) * (1.0 - pt);
                let rad1 = base_r + ease * (unit * 2.1);
                let a1 = ((1.0 - pt) * 235.0) as u32;
                draw_ring(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    kcx,
                    kcy,
                    rad1,
                    4.0,
                    (a1 << 24) | (ring_col & 0x00FF_FFFF),
                );
            }
        }

        // 2. Fixed 4-slot Mosaic dock in the upper glass window (drawn above effects)
        let has_any_tile = cfg
            .mosaic
            .tiles
            .iter()
            .any(|t| t.tile_type != TileType::None);
        if has_any_tile {
            let dock_x_f = self.phys_layout.mosaic_x;
            let dock_y = self.phys_layout.mosaic_y.round() as i32;
            let dock_h = self.phys_layout.mosaic_h.round() as i32;
            if !overlay_only && cfg.appearance.module_dimming > 1.0 {
                let dim_a =
                    ((cfg.appearance.module_dimming / 100.0) * 140.0).clamp(0.0, 180.0) as u32;
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    dock_x_f.round() as i32,
                    dock_y,
                    self.phys_layout.mosaic_w.round() as i32,
                    dock_h,
                    14,
                    (dim_a << 24) | 0x0007_090E,
                );
            }
            let gap_f = self.phys_layout.mosaic_gap;
            let tile_w_f = (self.phys_layout.mosaic_w - 3.0 * gap_f) / 4.0;
            for (slot, tile) in cfg.mosaic.tiles.iter().enumerate() {
                if tile.tile_type == TileType::None {
                    continue;
                }
                let x0_f = dock_x_f + (slot as f32) * (tile_w_f + gap_f);
                let x1_f = x0_f + tile_w_f;
                let tx = x0_f.round() as i32;
                let tw = (x1_f.round() as i32) - tx;
                self.render_mosaic_tile(
                    cfg, st, media_dir, slot, tile, tx, dock_y, tw, dock_h, now_sec,
                );
            }
        }

        // 3. Parallax-shifted key legends, shortcut overlays, and icons, drawn above effects so neighbor ripples do not occlude legends.
        let held_mods = st.held_modifier_mask(&cfg.keymap);
        for kr in &keys {
            let (dx, dy) = compute_key_parallax(
                &cfg.settings,
                kr.x as f64 + kr.w as f64 * 0.5,
                kr.y as f64 + kr.h as f64 * 0.5,
            );
            let kx = (kr.x + dx as f32) as i32;
            let ky = (kr.y + dy as f32) as i32;
            let kw = kr.w as i32;
            let kh = kr.h as i32;
            let kidx = kr.key_index as usize;
            let kcx = kx as f32 + kw as f32 * 0.5;
            let kcy = ky as f32 + kh as f32 * 0.5;

            let (legend, icon_opt, leg_col, _) = resolve_key_style(
                actions,
                fallback_actions,
                style_layer.as_ref(),
                cfg.appearance.legend_argb,
                cfg.appearance.ring_glow_argb,
                kidx,
            );
            let icon_owned = icon_opt.map(|s| s.to_string());

            let active_kc = actions
                .get(kidx)
                .filter(|a| a.key_code != "KC_TRANSPARENT")
                .or_else(|| fallback_actions.get(kidx))
                .map(|a| a.key_code.as_str())
                .unwrap_or("");
            // Lock Key Indicator (Caps Lock, Num Lock, Scroll Lock -> Off / Dot / Highlight)
            let is_lock_active = match active_kc {
                "KC_CAPS_LOCK" | "KC_CAPS" => st.caps_lock,
                "KC_NUM_LOCK" | "KC_NUM" => st.num_lock,
                "KC_SCROLL_LOCK" | "KC_SCRL" => st.scroll_lock,
                _ => false,
            };
            if is_lock_active {
                let lock_col = cfg
                    .appearance
                    .lock_indicator_argb
                    .unwrap_or(cfg.appearance.ring_glow_argb);
                match cfg.appearance.lock_indicator_style {
                    LockIndicatorStyle::Off => {}
                    LockIndicatorStyle::Dot => {
                        fill_circle_alpha(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            (kx + kw - 12) as f32,
                            (ky + 12) as f32,
                            5.0,
                            0xFF00_0000 | (lock_col & 0x00FF_FFFF),
                        );
                    }
                    LockIndicatorStyle::Highlight => {
                        fill_round_rect_alpha(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kx + 2,
                            ky + 2,
                            kw - 4,
                            kh - 4,
                            8,
                            0x5500_0000 | (lock_col & 0x00FF_FFFF),
                        );
                    }
                }
            }
            let shortcut_ov = if held_mods != 0 && !active_kc.is_empty() {
                st.shortcut_overlays
                    .iter()
                    .find(|ov| ov.mods == held_mods && ov.key_code == active_kc)
            } else {
                None
            };
            let is_shortcut = shortcut_ov.is_some();
            if is_shortcut && !legend.is_empty() {
                // Original binding inset cleanly inside the top-left safe zone without a background box.
                self.draw_text(cfg, media_dir, kx + 12, ky + 10, 11, 0x99E2_E8F0, &legend);
            }
            let display_legend: String = if let Some(sc) = shortcut_ov {
                sc.label.clone()
            } else {
                legend.clone()
            };
            let display_leg_col = if is_shortcut { 0xFF99_F6E4 } else { leg_col };
            // Never inherit original key's icon as the replacement shortcut's icon
            let eff_icon_owned = if is_shortcut {
                shortcut_ov
                    .filter(|sc| !sc.icon.is_empty())
                    .map(|sc| sc.icon.clone())
            } else {
                icon_owned
            };

            let mut drew_icon = false;
            if let Some(icon_name) = eff_icon_owned.as_deref() {
                let lower = icon_name.to_ascii_lowercase();
                let with_ext = if lower.ends_with(".png")
                    || lower.ends_with(".svg")
                    || lower.ends_with(".jpg")
                    || lower.ends_with(".jpeg")
                    || lower.ends_with(".bmp")
                    || lower.ends_with(".gif")
                {
                    icon_name.to_string()
                } else {
                    format!("{icon_name}.png")
                };
                let ipath = self
                    .assets
                    .resolve_asset_path_cached(media_dir, "key_icons", &with_ext)
                    .or_else(|| {
                        self.assets
                            .resolve_asset_path_cached(media_dir, "shortcuts", &with_ext)
                    });
                if let Some(ipath) = ipath {
                    let ip_s = ipath.to_string_lossy().to_string();
                    let isz = if is_shortcut {
                        (kw.min(kh) - 48).clamp(18, 26)
                    } else {
                        (kw.min(kh) - 22).max(16)
                    };
                    let iy = if is_shortcut {
                        (kcy as i32 - isz / 2 - 4).max(ky + 20)
                    } else {
                        kcy as i32 - isz / 2
                    };
                    let retint = cfg.appearance.retint_icons;
                    if let Some(kim) = self.assets.load_image(media_dir, &ip_s) {
                        draw_rgba_sprite(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            kcx as i32 - isz / 2,
                            iy,
                            isz,
                            isz,
                            &kim.rgba,
                            kim.width as i32,
                            kim.height as i32,
                            retint,
                            if is_shortcut {
                                display_leg_col
                            } else {
                                leg_col
                            },
                        );
                        drew_icon = true;
                    }
                }
            }
            if is_shortcut && !drew_icon {
                let gcx = kcx as i32;
                let gcy = (kcy as i32 - 5).max(ky + 30);
                let gsz = (kw.min(kh) - 48).clamp(20, 26);
                drew_icon = self.assets.draw_lucide_shortcut_svg(
                    &display_legend,
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gcx,
                    gcy,
                    gsz,
                    display_leg_col,
                ) || draw_shortcut_glyph(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gcx,
                    gcy,
                    &display_legend,
                    display_leg_col,
                );
            } else if !is_shortcut && !drew_icon && !active_kc.is_empty() {
                let has_explicit_label = style_layer
                    .as_ref()
                    .and_then(|sl| sl.keys.get(&kidx))
                    .and_then(|ko| ko.label.as_ref())
                    .is_some();
                if !has_explicit_label {
                    let gsz = (kw.min(kh) - 54).clamp(24, 34);
                    drew_icon = self.assets.draw_lucide_keycode_svg(
                        active_kc,
                        &mut self.pixels,
                        self.width,
                        self.height,
                        kcx as i32,
                        kcy as i32,
                        gsz,
                        leg_col,
                    );
                }
            }

            if (is_shortcut || !drew_icon) && !display_legend.is_empty() {
                let (text_cy, px_h) = if is_shortcut && drew_icon {
                    ((ky + kh - 21) as f32, 11)
                } else if is_shortcut {
                    (
                        kcy + 6.0,
                        match display_legend.chars().count() {
                            0..=4 => 15,
                            5..=7 => 13,
                            _ => 11,
                        },
                    )
                } else {
                    (
                        kcy,
                        match display_legend.chars().count() {
                            0..=2 => 27,
                            3..=4 => 22,
                            5..=6 => 18,
                            _ => 15,
                        },
                    )
                };
                if cfg.appearance.drop_shadow {
                    self.draw_text_centered(
                        cfg,
                        media_dir,
                        kcx + 1.5,
                        text_cy + 1.5,
                        px_h,
                        0xBB00_0000,
                        &display_legend,
                    );
                }
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    kcx,
                    text_cy,
                    px_h,
                    display_leg_col,
                    &display_legend,
                );
            }
        }

        // 4. Optional alignment grid lines
        if cfg.settings.show_alignment_grid {
            for x in (0..self.width).step_by(120) {
                draw_line(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    x,
                    0,
                    x,
                    self.height - 1,
                    0x3300_FF00,
                );
            }
            for y in (0..self.height).step_by(120) {
                draw_line(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    0,
                    y,
                    self.width - 1,
                    y,
                    0x3300_FF00,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        cfg: &DisplayConfig,
        media_dir: Option<&str>,
        x: i32,
        y: i32,
        px_h: i32,
        argb: u32,
        text: &str,
    ) {
        self.assets.draw_text(
            media_dir,
            &cfg.appearance.font_family,
            cfg.appearance.font_weight,
            cfg.appearance.font_italic,
            &mut self.pixels,
            self.width,
            self.height,
            x,
            y,
            px_h,
            argb,
            text,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text_centered(
        &mut self,
        cfg: &DisplayConfig,
        media_dir: Option<&str>,
        cx: f32,
        cy: f32,
        px_h: i32,
        argb: u32,
        text: &str,
    ) {
        let w = self.assets.measure_text(
            media_dir,
            &cfg.appearance.font_family,
            cfg.appearance.font_weight,
            cfg.appearance.font_italic,
            px_h,
            text,
        );
        self.draw_text(
            cfg,
            media_dir,
            cx as i32 - w / 2,
            cy as i32 - px_h / 2,
            px_h,
            argb,
            text,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text_right(
        &mut self,
        cfg: &DisplayConfig,
        media_dir: Option<&str>,
        rx: i32,
        y: i32,
        px_h: i32,
        argb: u32,
        text: &str,
    ) {
        let w = self.assets.measure_text(
            media_dir,
            &cfg.appearance.font_family,
            cfg.appearance.font_weight,
            cfg.appearance.font_italic,
            px_h,
            text,
        );
        self.draw_text(cfg, media_dir, rx - w, y, px_h, argb, text);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_mosaic_tile(
        &mut self,
        cfg: &DisplayConfig,
        st: &LiveState,
        media_dir: Option<&str>,
        tile_idx: usize,
        tile: &MosaicTileConfig,
        tx: i32,
        ty: i32,
        tw: i32,
        th: i32,
        now_sec: i64,
    ) {
        let accent = if cfg.appearance.retint_modules {
            cfg.appearance.ring_glow_argb
        } else {
            0xFF66_CCFF
        };
        let subtle = 0xD2DC_E8F5u32;
        let tm = local_tm(now_sec);

        match tile.tile_type {
            TileType::DigitalClock | TileType::MinimalClock => {
                // Digital clock tile: large light time, AM/PM indicator, and uppercase date line.
                let h12 = hour12(tm.hour);
                let time_s = format!("{}:{:02}", h12, tm.min);
                let ampm = if tm.hour >= 12 { "PM" } else { "AM" };
                let tw_time = self.assets.measure_text(
                    media_dir,
                    &cfg.appearance.font_family,
                    cfg.appearance.font_weight,
                    cfg.appearance.font_italic,
                    72,
                    &time_s,
                );
                let tw_ampm = self.assets.measure_text(
                    media_dir,
                    &cfg.appearance.font_family,
                    cfg.appearance.font_weight,
                    cfg.appearance.font_italic,
                    24,
                    ampm,
                );
                let total_w = tw_time + 8 + tw_ampm;
                let x_start = tx + (tw - total_w) / 2;
                let y_time = ty + th / 2 - 54;
                self.draw_text(cfg, media_dir, x_start, y_time, 72, 0xFFFF_FFFF, &time_s);
                self.draw_text(
                    cfg,
                    media_dir,
                    x_start + tw_time + 8,
                    y_time + 40,
                    24,
                    subtle,
                    ampm,
                );
                let date_s = format!(
                    "{}, {} {}, {}",
                    tm.weekday_upper(),
                    tm.month_upper(),
                    tm.mday,
                    tm.year
                );
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (tx + tw / 2) as f32,
                    (ty + th - 48) as f32,
                    16,
                    subtle,
                    &date_s,
                );
            }

            TileType::Weather => {
                // Weather tile: compact left stats column (city, city time, sunrise/sunset/AQI),
                // subtle vertical hairline divider, right weather readout + 3-hour forecast.
                let city = if tile.city.is_empty() {
                    &st.weather_city
                } else {
                    &tile.city
                };
                let left_right = tx + 148;
                let div_x = tx + 158;
                let ctm = if let Some(off_s) = st.weather_utc_offset_sec {
                    utc_tm(now_sec + off_s)
                } else {
                    local_tm(now_sec)
                };
                let h12 = hour12(ctm.hour);
                let tline = format!(
                    "{}:{:02} {}",
                    h12,
                    ctm.min,
                    if ctm.hour >= 12 { "PM" } else { "AM" }
                );

                self.draw_text(cfg, media_dir, tx + 12, ty + 14, 22, 0xFFFF_FFFF, city);
                self.draw_text(cfg, media_dir, tx + 12, ty + 42, 14, subtle, &tline);

                let sr = if st.weather_sunrise.is_empty() {
                    "--"
                } else {
                    st.weather_sunrise.as_str()
                };
                let ss = if st.weather_sunset.is_empty() {
                    "--"
                } else {
                    st.weather_sunset.as_str()
                };
                let aqi_s = st
                    .weather_aqi
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "--".into());
                let stat_y0 = ty + 88;
                self.draw_text(cfg, media_dir, tx + 12, stat_y0, 13, subtle, "Sunrise");
                self.draw_text_right(cfg, media_dir, left_right, stat_y0, 13, 0xFFFF_FFFF, sr);
                self.draw_text(cfg, media_dir, tx + 12, stat_y0 + 22, 13, subtle, "Sunset");
                self.draw_text_right(
                    cfg,
                    media_dir,
                    left_right,
                    stat_y0 + 22,
                    13,
                    0xFFFF_FFFF,
                    ss,
                );
                self.draw_text(cfg, media_dir, tx + 12, stat_y0 + 44, 13, subtle, "AQI");
                self.draw_text_right(
                    cfg,
                    media_dir,
                    left_right,
                    stat_y0 + 44,
                    13,
                    0xFFFF_FFFF,
                    &aqi_s,
                );

                fill_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    div_x,
                    ty + 16,
                    1,
                    th - 36,
                    0x1EFF_FFFF,
                );

                let rx = div_x + 16;
                let rw = (tx + tw - 8) - rx;
                let temp_s = if st.weather_ready {
                    format!("{:.1}°", st.weather_temp)
                } else {
                    "--°".into()
                };
                let hl_s = if st.weather_ready {
                    format!("{:.0}° / {:.0}°", st.weather_high, st.weather_low)
                } else {
                    "Syncing...".into()
                };
                self.draw_text(cfg, media_dir, rx, ty + 12, 42, 0xFFFF_FFFF, &temp_s);
                self.draw_text(cfg, media_dir, rx, ty + 60, 15, subtle, &hl_s);

                let icon_sz = 58;
                let icon_x = tx + tw - icon_sz - 14;
                let icon_y = ty + 10;
                if st.weather_ready {
                    if !self.assets.draw_weather_svg_with_day(
                        media_dir,
                        &st.weather_condition,
                        st.weather_is_day,
                        &mut self.pixels,
                        self.width,
                        self.height,
                        icon_x,
                        icon_y,
                        icon_sz,
                        accent,
                    ) {
                        draw_weather_fallback(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            icon_x + icon_sz / 2,
                            icon_y + icon_sz / 2,
                            &st.weather_condition,
                            accent,
                        );
                    }
                    let cols = st.weather_hourly.len().min(3);
                    if cols > 0 {
                        let col_w = rw / 3;
                        for (i, hf) in st.weather_hourly.iter().take(3).enumerate() {
                            let fcx = rx + col_w * (i as i32) + col_w / 2;
                            let f_isz = 30;
                            if !self.assets.draw_weather_svg_with_day(
                                media_dir,
                                &hf.condition,
                                hf.is_day,
                                &mut self.pixels,
                                self.width,
                                self.height,
                                fcx - f_isz / 2,
                                ty + th - 92,
                                f_isz,
                                0xFFE2_E8F0,
                            ) {
                                draw_weather_fallback(
                                    &mut self.pixels,
                                    self.width,
                                    self.height,
                                    fcx,
                                    ty + th - 77,
                                    &hf.condition,
                                    0xFFE2_E8F0,
                                );
                            }
                            let ft = format!("{:.0}°", hf.temp.round());
                            self.draw_text_centered(
                                cfg,
                                media_dir,
                                fcx as f32,
                                (ty + th - 46) as f32,
                                18,
                                0xFFFF_FFFF,
                                &ft,
                            );
                            self.draw_text_centered(
                                cfg,
                                media_dir,
                                fcx as f32,
                                (ty + th - 23) as f32,
                                13,
                                subtle,
                                &hf.hour_label,
                            );
                        }
                    } else {
                        self.draw_text(
                            cfg,
                            media_dir,
                            rx,
                            ty + th - 36,
                            15,
                            subtle,
                            st.weather_condition.as_str(),
                        );
                    }
                }
            }

            TileType::HwStatus => {
                // Hardware status tile: 3x2 hairline grid for CPU, GPU, Upload, Memory, VRAM, and Download.
                let cw = (tw - 16) / 3;
                let ch = (th - 16) / 2;
                let gx0 = tx + 8;
                let gy0 = ty + 8;
                draw_round_rect_outline(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gx0,
                    gy0,
                    cw * 3,
                    ch * 2,
                    4,
                    1,
                    0x26FF_FFFF,
                );
                fill_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gx0 + cw,
                    gy0,
                    1,
                    ch * 2,
                    0x26FF_FFFF,
                );
                fill_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gx0 + cw * 2,
                    gy0,
                    1,
                    ch * 2,
                    0x26FF_FFFF,
                );
                fill_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gx0,
                    gy0 + ch,
                    cw * 3,
                    1,
                    0x26FF_FFFF,
                );

                self.draw_text(cfg, media_dir, gx0 + 12, gy0 + 10, 14, subtle, "CPU");
                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + 12,
                    gy0 + 34,
                    30,
                    0xFFFF_FFFF,
                    &st.cpu_str,
                );

                self.draw_text(cfg, media_dir, gx0 + cw + 12, gy0 + 10, 14, subtle, "GPU");
                let gpu_s = if st.gpu_str.is_empty() {
                    "--"
                } else {
                    &st.gpu_str
                };
                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + cw + 12,
                    gy0 + 34,
                    30,
                    0xFFFF_FFFF,
                    gpu_s,
                );

                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + cw * 2 + 12,
                    gy0 + 10,
                    14,
                    subtle,
                    "Upload",
                );
                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + cw * 2 + 12,
                    gy0 + 36,
                    22,
                    0xFFFF_FFFF,
                    &st.net_up_str,
                );

                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + 12,
                    gy0 + ch + 10,
                    14,
                    subtle,
                    "Memory",
                );
                let mem_pct_s = format!("{:.0}%", st.mem_pct);
                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + 12,
                    gy0 + ch + 32,
                    28,
                    0xFFFF_FFFF,
                    &mem_pct_s,
                );
                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + 12,
                    gy0 + ch + 68,
                    13,
                    subtle,
                    &st.mem_str,
                );

                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + cw + 12,
                    gy0 + ch + 10,
                    14,
                    subtle,
                    "VRAM",
                );
                match (st.vram_used_gb, st.vram_total_gb) {
                    (Some(vu), Some(vt)) if vt > 0.0 => {
                        let vpct = ((vu / vt) * 100.0).clamp(0.0, 100.0);
                        let vpct_s = format!("{vpct:.0}%");
                        let vsub_s = format!("{vu:.1}/{vt:.1}G");
                        self.draw_text(
                            cfg,
                            media_dir,
                            gx0 + cw + 12,
                            gy0 + ch + 32,
                            28,
                            0xFFFF_FFFF,
                            &vpct_s,
                        );
                        self.draw_text(
                            cfg,
                            media_dir,
                            gx0 + cw + 12,
                            gy0 + ch + 68,
                            13,
                            subtle,
                            &vsub_s,
                        );
                    }
                    (Some(vu), _) if st.gpu_unified => {
                        let vuse_s = format!("{vu:.1}G");
                        self.draw_text(
                            cfg,
                            media_dir,
                            gx0 + cw + 12,
                            gy0 + ch + 34,
                            24,
                            0xFFFF_FFFF,
                            &vuse_s,
                        );
                        self.draw_text(
                            cfg,
                            media_dir,
                            gx0 + cw + 12,
                            gy0 + ch + 68,
                            13,
                            subtle,
                            "Unified",
                        );
                    }
                    _ => {
                        self.draw_text(
                            cfg,
                            media_dir,
                            gx0 + cw + 12,
                            gy0 + ch + 34,
                            24,
                            0xFF94_A3B8,
                            "--",
                        );
                        self.draw_text(
                            cfg,
                            media_dir,
                            gx0 + cw + 12,
                            gy0 + ch + 68,
                            13,
                            subtle,
                            if st.gpu_unified {
                                "Unified"
                            } else {
                                "Unavailable"
                            },
                        );
                    }
                }

                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + cw * 2 + 12,
                    gy0 + ch + 10,
                    14,
                    subtle,
                    "Download",
                );
                self.draw_text(
                    cfg,
                    media_dir,
                    gx0 + cw * 2 + 12,
                    gy0 + ch + 36,
                    22,
                    0xFFFF_FFFF,
                    &st.net_down_str,
                );
            }

            TileType::ActiveMedia => {
                // Active media tile: album artwork square on the left with title, artist, and progress on the right.
                let art_sz = (th - 36).clamp(80, 160);
                let ax = tx + 14;
                let ay = ty + (th - art_sz) / 2;
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    ax,
                    ay,
                    art_sz,
                    art_sz,
                    8,
                    0x4480_8898,
                );
                fill_circle_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    (ax + art_sz / 2) as f32,
                    (ay + art_sz / 2) as f32,
                    (art_sz as f32) * 0.28,
                    0x33FF_FFFF,
                );
                let rx = ax + art_sz + 18;
                let rw = (tx + tw - 16 - rx).max(40);
                let title = if st.media_title.is_empty() {
                    "No Active Media"
                } else {
                    &st.media_title
                };
                let artist = if st.media_artists.is_empty() {
                    "Paused"
                } else {
                    &st.media_artists
                };
                self.draw_text(cfg, media_dir, rx, ay + 12, 22, 0xFFFF_FFFF, title);
                self.draw_text(cfg, media_dir, rx, ay + 46, 17, subtle, artist);
                let frac = if st.media_length_s > 0 {
                    (st.media_played_s as f32 / st.media_length_s as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    rx,
                    ay + art_sz - 14,
                    rw,
                    5,
                    2,
                    0x33FF_FFFF,
                );
                if frac > 0.0 {
                    fill_round_rect_alpha(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        rx,
                        ay + art_sz - 14,
                        (frac * rw as f32) as i32,
                        5,
                        2,
                        accent,
                    );
                }
            }

            TileType::Pomodoro => {
                let secs = st.pomo_remaining_s;
                let total = if tile.pomo_main_min > 0 {
                    tile.pomo_main_min * 60
                } else {
                    25 * 60
                };
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 16, subtle, "POMODORO");
                let ts = format!("{:02}:{:02}", secs / 60, secs % 60);
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (tx + tw / 2) as f32,
                    (ty + th / 2 - 8) as f32,
                    54,
                    0xFFFF_FFFF,
                    &ts,
                );
                let bw = tw - 36;
                let fw = if total > 0 {
                    ((secs as f64 / total as f64) * bw as f64).min(bw as f64) as i32
                } else {
                    bw
                };
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    tx + 18,
                    ty + th - 48,
                    bw,
                    5,
                    2,
                    0x33FF_FFFF,
                );
                if fw > 0 {
                    fill_round_rect_alpha(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        tx + 18,
                        ty + th - 48,
                        fw,
                        5,
                        2,
                        accent,
                    );
                }
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (tx + tw / 2) as f32,
                    (ty + th - 26) as f32,
                    16,
                    if st.pomo_running { 0xFF77_FF99 } else { subtle },
                    if st.pomo_running { "RUNNING" } else { "READY" },
                );
            }

            TileType::WorldClock => {
                // World clock applies the tile's UTC offset directly without adding host timezone.
                let wt = utc_tm(now_sec + (tile.utc_offset_min as i64) * 60);
                let city = if !tile.city.is_empty() {
                    &tile.city
                } else if !tile.tz_label.is_empty() {
                    &tile.tz_label
                } else {
                    "UTC"
                };
                self.draw_text(cfg, media_dir, tx + 18, ty + 18, 18, subtle, city);
                let ts = format!("{:02}:{:02}", wt.hour, wt.min);
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (tx + tw / 2) as f32,
                    (ty + th / 2 - 6) as f32,
                    54,
                    0xFFFF_FFFF,
                    &ts,
                );
                let oh = tile.utc_offset_min / 60;
                let om = (tile.utc_offset_min % 60).abs();
                let sub = if om != 0 {
                    format!("UTC{oh:+}:{om:02}  {:02}/{:02}", wt.mon, wt.mday)
                } else {
                    format!("UTC{oh:+}  {:02}/{:02}", wt.mon, wt.mday)
                };
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (tx + tw / 2) as f32,
                    (ty + th - 32) as f32,
                    16,
                    subtle,
                    &sub,
                );
            }

            TileType::VirtualPet => {
                let pname = if tile.pet_name.is_empty() {
                    "Mochi"
                } else {
                    &tile.pet_name
                };
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 20, accent, pname);
                let mood_s = match st.pet_mood {
                    1 => "HAPPY",
                    2 => "NAP",
                    3 => "PLAY",
                    _ => "IDLE",
                };
                self.draw_text_right(cfg, media_dir, tx + tw - 18, ty + 18, 15, subtle, mood_s);
                let sprite_name = match st.pet_mood {
                    1 => "cat_happy.png",
                    2 => "cat_sleep.png",
                    _ => "cat_idle.png",
                };
                let mut drew = false;
                let sp = self
                    .assets
                    .resolve_asset_path_cached(media_dir, "pet", sprite_name)
                    .or_else(|| {
                        self.assets
                            .resolve_asset_path_cached(media_dir, "key_icons", sprite_name)
                    });
                if let Some(sp) = sp {
                    let sp_s = sp.to_string_lossy().to_string();
                    if let Some(im) = self.assets.load_image(media_dir, &sp_s) {
                        // Pet sprites use a two-frame horizontal strip when width >= 2 * height; otherwise draw the single frame full-width.
                        let is_two_frame = im.height > 0 && im.width >= im.height.saturating_mul(2);
                        let fw = if is_two_frame {
                            (im.width / 2).max(1) as i32
                        } else {
                            im.width.max(1) as i32
                        };
                        let fh = im.height.max(1) as i32;
                        let fidx = if is_two_frame {
                            (now_sec & 1) as i32
                        } else {
                            0
                        };
                        let ssz = 76;
                        draw_rgba_subsprite(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            tx + tw / 2 - ssz / 2,
                            ty + th / 2 - ssz / 2,
                            ssz,
                            ssz,
                            &im.rgba,
                            im.width as i32,
                            fidx * fw,
                            0,
                            fw,
                            fh,
                        );
                        drew = true;
                    }
                }
                if !drew {
                    draw_pet_sprite(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        tx + tw / 2,
                        ty + th / 2,
                        st.pet_mood,
                        now_sec,
                        accent,
                    );
                }
                let hs = format!("HAPPY {}%", st.pet_happiness);
                self.draw_text(cfg, media_dir, tx + 18, ty + th - 36, 16, subtle, &hs);
                let bw = (tw - 184).max(40);
                let fw = ((st.pet_happiness as f32 / 100.0) * bw as f32) as i32;
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    tx + 166,
                    ty + th - 28,
                    bw,
                    8,
                    4,
                    0x33FF_FFFF,
                );
                if fw > 0 {
                    fill_round_rect_alpha(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        tx + 166,
                        ty + th - 28,
                        fw,
                        8,
                        4,
                        accent,
                    );
                }
            }

            TileType::HwGraph => {
                let (hist, label, cur_s): (&[f32], &str, &str) = match tile.graph_metric.as_str() {
                    "gpu" => (
                        &st.gpu_hist,
                        "GPU",
                        if st.gpu_str.is_empty() {
                            "--"
                        } else {
                            &st.gpu_str
                        },
                    ),
                    "mem" => (&st.mem_hist, "MEM", &st.mem_str),
                    _ => (&st.cpu_hist, "CPU", &st.cpu_str),
                };
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, label);
                self.draw_text_right(
                    cfg,
                    media_dir,
                    tx + tw - 18,
                    ty + 16,
                    18,
                    0xFFFF_FFFF,
                    cur_s,
                );
                let gx = tx + 18;
                let gy = ty + 54;
                let gw = tw - 36;
                let gh = th - 74;
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gx,
                    gy,
                    gw,
                    gh,
                    8,
                    0x3300_0000,
                );
                draw_round_rect_outline(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    gx,
                    gy,
                    gw,
                    gh,
                    8,
                    1,
                    0x44FF_FFFF,
                );
                if !hist.is_empty() {
                    let step = (gw / HW_HIST_LEN as i32).max(4);
                    for (i, &pct) in hist.iter().enumerate() {
                        let bh = ((pct / 100.0) * (gh - 6) as f32).max(if pct > 0.0 {
                            2.0
                        } else {
                            0.0
                        }) as i32;
                        let bx = gx + 4 + (i as i32) * step;
                        let by = gy + gh - 2 - bh;
                        fill_rect_alpha(
                            &mut self.pixels,
                            self.width,
                            self.height,
                            bx,
                            by,
                            step - 3,
                            bh,
                            0x8800_0000 | (accent & 0x00FF_FFFF),
                        );
                    }
                }
            }

            TileType::AnalogClock => {
                let cx = (tx + tw / 2) as f32;
                let cy = (ty + th / 2) as f32;
                let rad = (tw.min(th) as f32) * 0.5 - 16.0;
                draw_ring(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    cx,
                    cy,
                    rad,
                    3.0,
                    accent,
                );
                let sa =
                    (tm.sec as f64 / 60.0) * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                let ma =
                    (tm.min as f64 / 60.0) * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                let ha = (((tm.hour % 12) as f64 + tm.min as f64 / 60.0) / 12.0)
                    * std::f64::consts::TAU
                    - std::f64::consts::FRAC_PI_2;
                draw_line(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    cx as i32,
                    cy as i32,
                    (cx as f64 + ha.cos() * rad as f64 * 0.52) as i32,
                    (cy as f64 + ha.sin() * rad as f64 * 0.52) as i32,
                    0xFFFF_FFFF,
                );
                draw_line(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    cx as i32,
                    cy as i32,
                    (cx as f64 + ma.cos() * rad as f64 * 0.78) as i32,
                    (cy as f64 + ma.sin() * rad as f64 * 0.78) as i32,
                    accent,
                );
                draw_line(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    cx as i32,
                    cy as i32,
                    (cx as f64 + sa.cos() * rad as f64 * 0.85) as i32,
                    (cy as f64 + sa.sin() * rad as f64 * 0.85) as i32,
                    0xFFFF_6666,
                );
            }

            TileType::FlipClock => {
                let cw = (tw - 60) / 2;
                let ch = th - 64;
                let cy0 = ty + 40;
                let lx = tx + 20;
                let rx = tx + 40 + cw;
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    lx,
                    cy0,
                    cw,
                    ch,
                    10,
                    0xCC18_1D2B,
                );
                fill_round_rect_alpha(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    rx,
                    cy0,
                    cw,
                    ch,
                    10,
                    0xCC18_1D2B,
                );
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (lx + cw / 2) as f32,
                    (cy0 + ch / 2) as f32,
                    52,
                    0xFFFF_FFFF,
                    &format!("{:02}", tm.hour),
                );
                self.draw_text_centered(
                    cfg,
                    media_dir,
                    (rx + cw / 2) as f32,
                    (cy0 + ch / 2) as f32,
                    52,
                    accent,
                    &format!("{:02}", tm.min),
                );
            }

            TileType::KbdState => {
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, "KEYBOARD");
                let info = format!(
                    "P{}  L{}  {}",
                    st.active_profile,
                    st.active_layer,
                    if cfg.keymap.selected_layout == FrameLayoutType::Iso {
                        "ISO"
                    } else {
                        "ANSI"
                    }
                );
                self.draw_text(cfg, media_dir, tx + 18, ty + 52, 20, 0xFFFF_FFFF, &info);
                for (li, (lbl, on)) in [
                    ("CAPS", st.caps_lock),
                    ("NUM", st.num_lock),
                    ("SCRL", st.scroll_lock),
                ]
                .iter()
                .enumerate()
                {
                    let lx = tx + 18 + (li as i32) * 96;
                    let ly = ty + 100;
                    fill_round_rect_alpha(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        lx,
                        ly,
                        84,
                        34,
                        8,
                        if *on {
                            0xCC00_0000 | (accent & 0x00FF_FFFF)
                        } else {
                            0x33FF_FFFF
                        },
                    );
                    self.draw_text_centered(
                        cfg,
                        media_dir,
                        (lx + 42) as f32,
                        (ly + 17) as f32,
                        16,
                        if *on { 0xFF0A_0D14 } else { subtle },
                        lbl,
                    );
                }
            }

            TileType::DebugTelemetry => {
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, "DEBUG");
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 52,
                    18,
                    0xFFFF_FFFF,
                    &format!("SEQ {}", st.key_seq),
                );
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 84,
                    16,
                    subtle,
                    &format!(
                        "VID {} ({})",
                        if self.bg.is_video { "ON" } else { "OFF" },
                        self.bg.video_frames_rendered
                    ),
                );
            }

            TileType::DeviceInfo => {
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, "DEVICE");
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 52,
                    22,
                    0xFFFF_FFFF,
                    if cfg.keymap.selected_layout == FrameLayoutType::Iso {
                        "ISO 87"
                    } else {
                        "ANSI 85"
                    },
                );
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 90,
                    16,
                    subtle,
                    &format!(
                        "P{}  L{}  M{}",
                        st.active_profile, st.active_layer, st.active_actuation_mode
                    ),
                );
            }

            TileType::QcSensor => {
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, "QC SENSOR");
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 52,
                    20,
                    0xFFFF_FFFF,
                    if st.qc_mode { "ACTIVE" } else { "STANDBY" },
                );
            }

            TileType::DisplayPrefs => {
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, "DISPLAY");
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 52,
                    18,
                    0xFFFF_FFFF,
                    &cfg.appearance.font_family,
                );
                self.draw_text(
                    cfg,
                    media_dir,
                    tx + 18,
                    ty + 84,
                    16,
                    subtle,
                    if cfg.settings.parallax_active {
                        "PARALLAX ON"
                    } else {
                        "PARALLAX OFF"
                    },
                );
            }

            TileType::Generic => {
                let gs = &st.generic_slots[tile_idx];
                let use_slot = gs.active;
                let title = if use_slot && !gs.title.is_empty() {
                    &gs.title
                } else if st.has_generic_live && !st.generic_title.is_empty() {
                    &st.generic_title
                } else if !tile.generic_title.is_empty() {
                    &tile.generic_title
                } else {
                    "MODULE"
                };
                let val = if use_slot && !gs.value.is_empty() {
                    &gs.value
                } else if st.has_generic_live && !st.generic_value.is_empty() {
                    &st.generic_value
                } else if !tile.generic_value.is_empty() {
                    &tile.generic_value
                } else {
                    "--"
                };
                let sub = if use_slot && !gs.subtitle.is_empty() {
                    &gs.subtitle
                } else if st.has_generic_live && !st.generic_subtitle.is_empty() {
                    &st.generic_subtitle
                } else {
                    &tile.generic_subtitle
                };
                let prog = if use_slot {
                    gs.progress
                } else if st.has_generic_live {
                    st.generic_progress
                } else {
                    tile.generic_progress
                };
                self.draw_text(cfg, media_dir, tx + 18, ty + 16, 18, accent, title);
                self.draw_text(cfg, media_dir, tx + 18, ty + 52, 34, 0xFFFF_FFFF, val);
                if !sub.is_empty() {
                    self.draw_text(cfg, media_dir, tx + 18, ty + 104, 16, subtle, sub);
                }
                if prog > 0.0 {
                    let bw = tw - 36;
                    fill_round_rect_alpha(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        tx + 18,
                        ty + th - 26,
                        bw,
                        6,
                        3,
                        0x33FF_FFFF,
                    );
                    fill_round_rect_alpha(
                        &mut self.pixels,
                        self.width,
                        self.height,
                        tx + 18,
                        ty + th - 26,
                        (prog.clamp(0.0, 1.0) * bw as f64) as i32,
                        6,
                        3,
                        accent,
                    );
                }
            }

            TileType::None => {}
        }
    }
}

struct SimpleTm {
    year: i32,
    mon: u32,
    mday: u32,
    wday: u32,
    hour: u32,
    min: u32,
    sec: u32,
}

impl SimpleTm {
    fn weekday_upper(&self) -> &'static str {
        match self.wday {
            0 => "SUNDAY",
            1 => "MONDAY",
            2 => "TUESDAY",
            3 => "WEDNESDAY",
            4 => "THURSDAY",
            5 => "FRIDAY",
            _ => "SATURDAY",
        }
    }
    fn month_upper(&self) -> &'static str {
        match self.mon {
            1 => "JANUARY",
            2 => "FEBRUARY",
            3 => "MARCH",
            4 => "APRIL",
            5 => "MAY",
            6 => "JUNE",
            7 => "JULY",
            8 => "AUGUST",
            9 => "SEPTEMBER",
            10 => "OCTOBER",
            11 => "NOVEMBER",
            _ => "DECEMBER",
        }
    }
}

fn utc_tm(unix_sec: i64) -> SimpleTm {
    // Howard Hinnant civil-from-days algorithm: exact proleptic Gregorian UTC date + weekday on every OS.
    let days = unix_sec.div_euclid(86400);
    let secs = unix_sec.rem_euclid(86400) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    let wday = (days + 4).rem_euclid(7) as u32;
    SimpleTm {
        year,
        mon: m as u32,
        mday: d as u32,
        wday,
        hour: secs / 3600,
        min: (secs % 3600) / 60,
        sec: secs % 60,
    }
}

fn local_tm(unix_sec: i64) -> SimpleTm {
    #[cfg(unix)]
    unsafe {
        let t = unix_sec as libc::time_t;
        let mut out: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut out).is_null() {
            return SimpleTm {
                year: out.tm_year + 1900,
                mon: (out.tm_mon + 1) as u32,
                mday: out.tm_mday as u32,
                wday: out.tm_wday as u32,
                hour: out.tm_hour as u32,
                min: out.tm_min as u32,
                sec: out.tm_sec as u32,
            };
        }
    }
    #[cfg(target_os = "windows")]
    unsafe {
        #[repr(C)]
        #[derive(Default)]
        struct WinTm {
            tm_sec: i32,
            tm_min: i32,
            tm_hour: i32,
            tm_mday: i32,
            tm_mon: i32,
            tm_year: i32,
            tm_wday: i32,
            tm_yday: i32,
            tm_isdst: i32,
        }
        extern "C" {
            fn _localtime64_s(tm: *mut WinTm, time: *const i64) -> i32;
        }
        let t = unix_sec;
        let mut out = WinTm::default();
        if _localtime64_s(&mut out, &t) == 0 {
            return SimpleTm {
                year: out.tm_year + 1900,
                mon: (out.tm_mon + 1) as u32,
                mday: out.tm_mday as u32,
                wday: out.tm_wday as u32,
                hour: out.tm_hour as u32,
                min: out.tm_min as u32,
                sec: out.tm_sec as u32,
            };
        }
    }
    utc_tm(unix_sec)
}

#[inline]
fn blend_px(dst: &mut u32, src: u32) {
    let sa = (src >> 24) & 0xFF;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        *dst = src;
        return;
    }
    blend_argb_cov(dst, src | 0xFF00_0000, sa as u8);
}

#[allow(clippy::too_many_arguments)]
fn fill_rect_alpha(pix: &mut [u32], bw: i32, bh: i32, x: i32, y: i32, w: i32, h: i32, col: u32) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(bw);
    let y1 = (y + h).min(bh);
    for py in y0..y1 {
        let row = (py * bw) as usize;
        for px in x0..x1 {
            blend_px(&mut pix[row + px as usize], col);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_round_rect_alpha(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    col: u32,
) {
    let rad = r.min(w / 2).min(h / 2).max(0);
    let r2 = rad * rad;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(bw);
    let y1 = (y + h).min(bh);
    for py in y0..y1 {
        let row = (py * bw) as usize;
        let dy = if py < y + rad {
            (y + rad) - py
        } else if py >= y + h - rad {
            py - (y + h - rad - 1)
        } else {
            0
        };
        for px in x0..x1 {
            let dx = if px < x + rad {
                (x + rad) - px
            } else if px >= x + w - rad {
                px - (x + w - rad - 1)
            } else {
                0
            };
            if dx > 0 && dy > 0 && (dx * dx + dy * dy) > r2 {
                continue;
            }
            blend_px(&mut pix[row + px as usize], col);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_round_rect_outline(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    thick: i32,
    col: u32,
) {
    let rad = r.min(w / 2).min(h / 2).max(0);
    let r_out = rad as f32;
    let r_in = (rad - thick).max(0) as f32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(bw);
    let y1 = (y + h).min(bh);
    for py in y0..y1 {
        let row = (py * bw) as usize;
        let in_y_edge = py < y + thick || py >= y + h - thick;
        let dy = if py < y + rad {
            (y + rad - py) as f32
        } else if py >= y + h - rad {
            (py - (y + h - rad - 1)) as f32
        } else {
            -1.0
        };
        for px in x0..x1 {
            let dx = if px < x + rad {
                (x + rad - px) as f32
            } else if px >= x + w - rad {
                (px - (x + w - rad - 1)) as f32
            } else {
                -1.0
            };
            let draw = if dx >= 0.0 && dy >= 0.0 {
                let d = (dx * dx + dy * dy).sqrt();
                d <= r_out && d >= r_in
            } else {
                in_y_edge || px < x + thick || px >= x + w - thick
            };
            if draw {
                blend_px(&mut pix[row + px as usize], col);
            }
        }
    }
}

fn fill_circle_alpha(pix: &mut [u32], bw: i32, bh: i32, cx: f32, cy: f32, rad: f32, col: u32) {
    let x0 = (cx - rad).floor().max(0.0) as i32;
    let y0 = (cy - rad).floor().max(0.0) as i32;
    let x1 = (cx + rad).ceil().min((bw - 1) as f32) as i32;
    let y1 = (cy + rad).ceil().min((bh - 1) as f32) as i32;
    let r2 = rad * rad;
    for py in y0..=y1 {
        let dy = py as f32 - cy;
        let row = (py * bw) as usize;
        for px in x0..=x1 {
            let dx = px as f32 - cx;
            if dx * dx + dy * dy <= r2 {
                blend_px(&mut pix[row + px as usize], col);
            }
        }
    }
}

fn lerp_argb_rgb(col: u32, target_rgb: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let r0 = ((col >> 16) & 0xFF) as f32;
    let g0 = ((col >> 8) & 0xFF) as f32;
    let b0 = (col & 0xFF) as f32;
    let r1 = ((target_rgb >> 16) & 0xFF) as f32;
    let g1 = ((target_rgb >> 8) & 0xFF) as f32;
    let b1 = (target_rgb & 0xFF) as f32;
    let r = (r0 + (r1 - r0) * t) as u32;
    let g = (g0 + (g1 - g0) * t) as u32;
    let b = (b0 + (b1 - b0) * t) as u32;
    (r << 16) | (g << 8) | b
}

#[allow(clippy::too_many_arguments)]
fn draw_autofocus_lock(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    kcx: f32,
    kcy: f32,
    kw: f32,
    kh: f32,
    age_ms: f32,
    rel_age_ms: f32,
    glow: f32,
    ring_col: u32,
) {
    // Frame the key cleanly with `0.78 * kw` so brackets do not overlap neighboring key legends.
    let pitch_x = (kw * 0.78).max(56.0);
    let pitch_y = (kh * 0.78).max(56.0);

    // Stage 1: Fast horizontal reticle scan + crisp pressed key halo
    let scan_t = (age_ms / 180.0).clamp(0.0, 1.0);
    let scan_half = pitch_x * (0.45 + 0.72 * (1.0 - (1.0 - scan_t) * (1.0 - scan_t)));
    let scan_alpha = ((1.0 - scan_t * 0.72) * 195.0 * glow) as u32;
    if scan_alpha > 4 {
        let scan_col =
            (scan_alpha << 24) | lerp_argb_rgb(ring_col, 0x00FF_FFFF, (1.0 - scan_t) * 0.6);
        let sy = (kcy - 1.5).round() as i32;
        let left_x0 = (kcx - scan_half).round() as i32;
        let left_x1 = (kcx - kw * 0.42).round() as i32;
        if left_x1 > left_x0 {
            fill_rect_alpha(pix, bw, bh, left_x0, sy, left_x1 - left_x0, 3, scan_col);
        }
        let right_x0 = (kcx + kw * 0.42).round() as i32;
        let right_x1 = (kcx + scan_half).round() as i32;
        if right_x1 > right_x0 {
            fill_rect_alpha(pix, bw, bh, right_x0, sy, right_x1 - right_x0, 3, scan_col);
        }
    }

    // Pressed key core bloom & subtle keycap outline
    let core_a = (glow * 70.0) as u32;
    fill_round_rect_alpha(
        pix,
        bw,
        bh,
        (kcx - kw * 0.46) as i32,
        (kcy - kh * 0.46) as i32,
        (kw * 0.92) as i32,
        (kh * 0.92) as i32,
        8,
        (core_a << 24) | (ring_col & 0x00FF_FFFF),
    );
    draw_round_rect_outline(
        pix,
        bw,
        bh,
        (kcx - kw * 0.48) as i32,
        (kcy - kh * 0.48) as i32,
        (kw * 0.96) as i32,
        (kh * 0.96) as i32,
        8,
        2,
        (((glow * 210.0) as u32) << 24) | (ring_col & 0x00FF_FFFF),
    );

    // Stage 2: corner-bracket hunt and lock (four L-brackets framing the pressed key)
    let scale = if age_ms < 110.0 {
        let u = (age_ms / 110.0).clamp(0.0, 1.0);
        1.36 - 0.44 * (1.0 - (1.0 - u) * (1.0 - u))
    } else if age_ms < 230.0 {
        let u = ((age_ms - 110.0) / 120.0).clamp(0.0, 1.0);
        0.92 + 0.08 * (1.0 - (1.0 - u) * (1.0 - u))
    } else {
        1.00 + (rel_age_ms / 480.0).clamp(0.0, 1.0) * 0.14
    };

    // Brief focus-lock white flash right as brackets snap to 1.00
    let lock_flash = if (205.0..=320.0).contains(&age_ms) {
        let dist = ((age_ms - 245.0) / 55.0).abs().clamp(0.0, 1.0);
        (1.0 - dist) * 0.82
    } else {
        0.0
    };

    let hw = pitch_x * scale;
    let hh = pitch_y * scale;
    let x0 = (kcx - hw).round() as i32;
    let y0 = (kcy - hh).round() as i32;
    let x1 = (kcx + hw).round() as i32;
    let y1 = (kcy + hh).round() as i32;

    let arm = (kw * 0.28).round().clamp(14.0, 26.0) as i32;
    let thick = 4;
    let rgb = lerp_argb_rgb(ring_col, 0x00FF_FFFF, lock_flash);
    let halo_a = (glow * 80.0) as u32;
    let main_a = (glow * 240.0) as u32;
    let halo_col = (halo_a << 24) | (ring_col & 0x00FF_FFFF);
    let main_col = (main_a << 24) | rgb;

    // 4 L-shaped corner brackets: (corner_x, corner_y, dx_sign, dy_sign)
    let corners = [
        (x0, y0, 1, 1),
        (x1, y0, -1, 1),
        (x0, y1, 1, -1),
        (x1, y1, -1, -1),
    ];
    for (cx, cy, sx, sy) in corners {
        let hx = if sx > 0 { cx - 1 } else { cx - arm - 1 };
        let hy = if sy > 0 { cy - 1 } else { cy - thick - 1 };
        fill_rect_alpha(pix, bw, bh, hx, hy, arm + 2, thick + 2, halo_col);
        let vx = if sx > 0 { cx - 1 } else { cx - thick - 1 };
        let vy = if sy > 0 { cy - 1 } else { cy - arm - 1 };
        fill_rect_alpha(pix, bw, bh, vx, vy, thick + 2, arm + 2, halo_col);

        let mx = if sx > 0 { cx } else { cx - arm };
        let my = if sy > 0 { cy } else { cy - thick };
        fill_rect_alpha(pix, bw, bh, mx, my, arm, thick, main_col);
        let mvx = if sx > 0 { cx } else { cx - thick };
        let mvy = if sy > 0 { cy } else { cy - arm };
        fill_rect_alpha(pix, bw, bh, mvx, mvy, thick, arm, main_col);
    }

    // Short horizontal ticks flanking the left and right edges at y = kcy.
    let tick_w = 10;
    let tick_h = 3;
    let ty = (kcy - 1.0).round() as i32;
    fill_rect_alpha(pix, bw, bh, x0 - 2, ty, tick_w, tick_h, main_col);
    fill_rect_alpha(pix, bw, bh, x1 - tick_w + 2, ty, tick_w, tick_h, main_col);
}

fn hour12(hour: u32) -> u32 {
    if hour.is_multiple_of(12) {
        12
    } else {
        hour % 12
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ring(pix: &mut [u32], bw: i32, bh: i32, cx: f32, cy: f32, rad: f32, thick: f32, col: u32) {
    draw_ring_arc(pix, bw, bh, cx, cy, rad, thick, col, false);
}

#[allow(clippy::too_many_arguments)]
fn draw_ring_arc(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    cx: f32,
    cy: f32,
    rad: f32,
    thick: f32,
    col: u32,
    omit_bottom_left: bool,
) {
    let base_a = (col >> 24) & 0xFF;
    if base_a == 0 {
        return;
    }
    let half = thick * 0.5;
    let x0 = (cx - rad - half - 1.0).floor().max(0.0) as i32;
    let y0 = (cy - rad - half - 1.0).floor().max(0.0) as i32;
    let x1 = (cx + rad + half + 1.0).ceil().min((bw - 1) as f32) as i32;
    let y1 = (cy + rad + half + 1.0).ceil().min((bh - 1) as f32) as i32;
    for py in y0..=y1 {
        let dy = py as f32 - cy;
        let row = (py * bw) as usize;
        for px in x0..=x1 {
            let dx = px as f32 - cx;
            if omit_bottom_left && dx < 0.0 && dy > 0.0 {
                continue;
            }
            let dist = ((dx * dx + dy * dy).sqrt() - rad).abs();
            if dist <= half + 1.0 {
                let cov = if dist <= half {
                    1.0
                } else {
                    1.0 - (dist - half)
                };
                let a = ((base_a as f32) * cov).clamp(0.0, 255.0) as u8;
                blend_argb_cov(&mut pix[row + px as usize], col | 0xFF00_0000, a);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    col: u32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x0 >= 0 && x0 < bw && y0 >= 0 && y0 < bh {
            blend_px(&mut pix[(y0 * bw + x0) as usize], col);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rgba_sprite(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    rgba: &[u8],
    sw: i32,
    sh: i32,
    retint: bool,
    tint_argb: u32,
) {
    draw_rgba_subsprite_opt(
        pix, bw, bh, dx, dy, dw, dh, rgba, sw, 0, 0, sw, sh, retint, tint_argb,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_rgba_subsprite(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    rgba: &[u8],
    stride_w: i32,
    sx0: i32,
    sy0: i32,
    sw: i32,
    sh: i32,
) {
    draw_rgba_subsprite_opt(
        pix, bw, bh, dx, dy, dw, dh, rgba, stride_w, sx0, sy0, sw, sh, false, 0,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_rgba_subsprite_opt(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    rgba: &[u8],
    stride_w: i32,
    sx0: i32,
    sy0: i32,
    sw: i32,
    sh: i32,
    retint: bool,
    tint_argb: u32,
) {
    for py in 0..dh {
        let oy = dy + py;
        if oy < 0 || oy >= bh {
            continue;
        }
        let sy = sy0 + (py * sh) / dh.max(1);
        let row = (oy * bw) as usize;
        for px in 0..dw {
            let ox = dx + px;
            if ox < 0 || ox >= bw {
                continue;
            }
            let sx = sx0 + (px * sw) / dw.max(1);
            let off = ((sy * stride_w + sx) * 4) as usize;
            if off + 3 >= rgba.len() {
                continue;
            }
            let sa = rgba[off + 3];
            if sa == 0 {
                continue;
            }
            let src = if retint {
                tint_argb | 0xFF00_0000
            } else {
                0xFF00_0000
                    | ((rgba[off] as u32) << 16)
                    | ((rgba[off + 1] as u32) << 8)
                    | (rgba[off + 2] as u32)
            };
            blend_argb_cov(&mut pix[row + ox as usize], src, sa);
        }
    }
}

fn draw_weather_fallback(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    cx: i32,
    cy: i32,
    cond: &str,
    col: u32,
) {
    let lc = cond.to_ascii_lowercase();
    if lc.contains("cloud") {
        fill_circle_alpha(pix, bw, bh, (cx - 10) as f32, (cy + 2) as f32, 11.0, col);
        fill_circle_alpha(pix, bw, bh, (cx + 2) as f32, (cy - 4) as f32, 14.0, col);
        fill_circle_alpha(pix, bw, bh, (cx + 14) as f32, (cy + 3) as f32, 10.0, col);
    } else if lc.contains("night") {
        fill_circle_alpha(pix, bw, bh, cx as f32, cy as f32, 16.0, col);
        fill_circle_alpha(
            pix,
            bw,
            bh,
            (cx + 7) as f32,
            (cy - 5) as f32,
            13.0,
            0xFF0A_0D18,
        );
    } else {
        fill_circle_alpha(pix, bw, bh, cx as f32, cy as f32, 13.0, col);
        draw_ring(pix, bw, bh, cx as f32, cy as f32, 21.0, 2.0, 0x99FF_FFFF);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_pet_sprite(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    cx: i32,
    cy: i32,
    mood: u8,
    now_sec: i64,
    accent: u32,
) {
    let bob = if mood == 1 && (now_sec & 1) == 0 {
        -4
    } else {
        0
    };
    let px = cx;
    let py = cy + bob;
    fill_round_rect_alpha(pix, bw, bh, px - 24, py - 10, 48, 30, 8, accent);
    fill_round_rect_alpha(pix, bw, bh, px - 18, py - 24, 36, 22, 6, accent);
    fill_rect_alpha(pix, bw, bh, px - 10, py - 16, 5, 5, 0xFF0A_0D14);
    fill_rect_alpha(pix, bw, bh, px + 5, py - 16, 5, 5, 0xFF0A_0D14);
}

fn draw_shortcut_glyph(
    pix: &mut [u32],
    bw: i32,
    bh: i32,
    cx: i32,
    cy: i32,
    label: &str,
    col: u32,
) -> bool {
    let has_word = |hay: &str, w: &str| {
        hay.split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|tok| tok == w)
    };
    let l = label.to_ascii_lowercase();
    let c = col | 0xFF00_0000;
    if l.contains("copy") || l.contains("dup") {
        draw_round_rect_outline(pix, bw, bh, cx - 9, cy - 10, 13, 15, 2, 2, c);
        draw_round_rect_outline(pix, bw, bh, cx - 4, cy - 5, 13, 15, 2, 2, c);
        true
    } else if l.contains("paste") {
        draw_round_rect_outline(pix, bw, bh, cx - 9, cy - 9, 18, 20, 3, 2, c);
        fill_rect_alpha(pix, bw, bh, cx - 4, cy - 12, 8, 4, c);
        fill_rect_alpha(pix, bw, bh, cx - 5, cy - 1, 10, 2, c);
        fill_rect_alpha(pix, bw, bh, cx - 5, cy + 4, 8, 2, c);
        true
    } else if has_word(&l, "cut") {
        draw_ring(pix, bw, bh, (cx - 6) as f32, (cy + 5) as f32, 4.0, 2.0, c);
        draw_ring(pix, bw, bh, (cx + 6) as f32, (cy + 5) as f32, 4.0, 2.0, c);
        draw_line(pix, bw, bh, cx - 4, cy + 2, cx + 6, cy - 10, c);
        draw_line(pix, bw, bh, cx + 4, cy + 2, cx - 6, cy - 10, c);
        true
    } else if l.contains("undo") {
        draw_ring_arc(
            pix,
            bw,
            bh,
            (cx + 1) as f32,
            (cy + 2) as f32,
            8.5,
            2.0,
            c,
            true,
        );
        draw_line(pix, bw, bh, cx - 8, cy - 4, cx - 3, cy - 9, c);
        draw_line(pix, bw, bh, cx - 8, cy - 4, cx - 2, cy - 1, c);
        true
    } else if l.contains("redo") {
        draw_ring(pix, bw, bh, (cx - 1) as f32, (cy + 2) as f32, 8.5, 2.0, c);
        draw_line(pix, bw, bh, cx + 8, cy - 4, cx + 3, cy - 9, c);
        draw_line(pix, bw, bh, cx + 8, cy - 4, cx + 2, cy - 1, c);
        true
    } else if l.contains("save") {
        draw_round_rect_outline(pix, bw, bh, cx - 10, cy - 10, 20, 20, 3, 2, c);
        fill_rect_alpha(pix, bw, bh, cx - 5, cy - 9, 10, 6, c);
        draw_round_rect_outline(pix, bw, bh, cx - 6, cy + 1, 12, 8, 1, 2, c);
        true
    } else if l.contains("find") || l.contains("search") {
        draw_ring(pix, bw, bh, (cx - 2) as f32, (cy - 2) as f32, 7.0, 2.2, c);
        draw_line(pix, bw, bh, cx + 3, cy + 3, cx + 10, cy + 10, c);
        draw_line(pix, bw, bh, cx + 4, cy + 3, cx + 11, cy + 10, c);
        true
    } else if l == "all" || l.contains("select") {
        draw_round_rect_outline(pix, bw, bh, cx - 10, cy - 9, 20, 18, 3, 2, c);
        fill_rect_alpha(
            pix,
            bw,
            bh,
            cx - 6,
            cy - 5,
            12,
            10,
            (0x55 << 24) | (c & 0x00FF_FFFF),
        );
        true
    } else if l.contains("close") {
        draw_line(pix, bw, bh, cx - 8, cy - 8, cx + 8, cy + 8, c);
        draw_line(pix, bw, bh, cx + 8, cy - 8, cx - 8, cy + 8, c);
        true
    } else if l == "new" {
        fill_rect_alpha(pix, bw, bh, cx - 9, cy - 1, 18, 3, c);
        fill_rect_alpha(pix, bw, bh, cx - 1, cy - 9, 3, 18, c);
        true
    } else if l.contains("tab") {
        draw_round_rect_outline(pix, bw, bh, cx - 11, cy - 8, 22, 16, 3, 2, c);
        fill_rect_alpha(pix, bw, bh, cx - 9, cy - 4, 8, 3, c);
        true
    } else if l.contains("quit") {
        draw_ring(pix, bw, bh, cx as f32, (cy + 1) as f32, 8.5, 2.2, c);
        fill_rect_alpha(pix, bw, bh, cx - 1, cy - 11, 3, 10, c);
        true
    } else if l.contains("reload") || l.contains("refresh") {
        draw_ring(pix, bw, bh, cx as f32, cy as f32, 8.5, 2.2, c);
        draw_line(pix, bw, bh, cx + 7, cy - 6, cx + 10, cy - 1, c);
        draw_line(pix, bw, bh, cx + 7, cy - 6, cx + 3, cy - 3, c);
        true
    } else {
        false
    }
}
