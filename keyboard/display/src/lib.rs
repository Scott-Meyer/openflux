pub mod assets;
pub mod config;
pub mod device_paths;
pub mod ipc;
pub mod layout;
pub mod media_bg;
pub mod renderer;
#[cfg(target_os = "linux")]
pub mod rkmpp;
#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(target_os = "linux")]
pub mod wayland_egl;

pub use config::{DisplayConfig, LiveState};
pub use layout::{FluxKeyRect, FrameLayoutType, DISPLAY_HEIGHT, DISPLAY_WIDTH};
pub use media_bg::{FfmpegVideoAdapter, VideoAdapter, VideoStream};
pub use renderer::Renderer;

use std::path::Path;
use std::sync::Arc;

pub struct DisplayPreview {
    pub cfg: DisplayConfig,
    pub state: LiveState,
    pub renderer: Renderer,
    pub media_dir: Option<String>,
}

impl DisplayPreview {
    pub fn new(width: i32, height: i32) -> Self {
        Self::with_ffmpeg(width, height, None)
    }

    pub fn with_ffmpeg(width: i32, height: i32, ffmpeg_bin: Option<std::path::PathBuf>) -> Self {
        let cfg = DisplayConfig::default();
        let state = LiveState::from_config(&cfg);
        let mut renderer = Renderer::new(width, height);
        renderer
            .bg
            .set_video_adapter(Some(Arc::new(FfmpegVideoAdapter {
                sync_first_frame: true,
                target_scale: Some((960, 540)),
                decoder_bin: None,
                ffmpeg_bin,
            })));
        Self {
            cfg,
            state,
            renderer,
            media_dir: None,
        }
    }

    pub fn dimensions(&self) -> (usize, usize) {
        (self.renderer.width as usize, self.renderer.height as usize)
    }

    pub fn load_config_dir(&mut self, config_dir: &str, media_dir: &str) -> Result<(), String> {
        let dir = Path::new(config_dir);
        if !dir.exists() {
            return Err(format!("Config dir does not exist: {config_dir}"));
        }
        self.cfg = DisplayConfig::load_dir(dir);
        if !media_dir.is_empty() {
            self.media_dir = Some(media_dir.to_string());
        }
        self.cfg.reconcile_live_theme(&self.state);
        self.reconcile_config_derived_state();
        Ok(())
    }

    pub fn set_config_json(
        &mut self,
        appearance_json: Option<&str>,
        settings_json: Option<&str>,
        keymap_json: Option<&str>,
        mosaic_json: Option<&str>,
        media_dir: Option<&str>,
    ) -> Result<(), String> {
        let mut cfg = DisplayConfig::default();
        if let Some(a) = appearance_json {
            cfg.load_appearance_json(a);
        }
        if let Some(s) = settings_json {
            cfg.load_settings_json(s);
        }
        if let Some(k) = keymap_json {
            cfg.load_keymap_json(k);
        }
        if let Some(m) = mosaic_json {
            cfg.load_mosaic_json(m);
        }
        self.cfg = cfg;
        if let Some(md) = media_dir {
            self.media_dir = Some(md.to_string());
        }
        // Clear any prior cached hardware theme intent when reloading draft config JSON for preview
        self.state.active_theme_id.clear();
        self.state.has_active_theme_for_profile = false;
        self.cfg.reconcile_live_theme(&self.state);
        self.reconcile_config_derived_state();
        Ok(())
    }

    fn reconcile_config_derived_state(&mut self) {
        if self.renderer.cached_layout_type != self.cfg.keymap.selected_layout {
            self.renderer.phys_layout =
                layout::build_physical_layout(self.cfg.keymap.selected_layout);
            self.renderer.cached_layout_type = self.cfg.keymap.selected_layout;
        }
        if !self.state.has_firmware_layer {
            self.state.active_layer = self.cfg.keymap.default_layer;
        }
        if !self.state.has_live_pomo && !self.state.pomo_running {
            let pomo_mins = self
                .cfg
                .mosaic
                .tiles
                .iter()
                .find(|t| t.tile_type == config::TileType::Pomodoro && t.pomo_main_min > 0)
                .map(|t| t.pomo_main_min)
                .unwrap_or(25);
            self.state.pomo_remaining_s = pomo_mins * 60;
        }
        if !self.state.has_live_weather_unit {
            for t in &self.cfg.mosaic.tiles {
                if t.tile_type == config::TileType::Weather && !t.city.is_empty() {
                    if !self.state.weather_ready {
                        self.state.weather_city = t.city.clone();
                    }
                    self.state.weather_celsius = t.celsius;
                }
            }
        }
        if self.renderer.bg.has_submitted_video
            && self.renderer.bg.submitted_bg_path != self.cfg.appearance.bg_path
        {
            self.renderer.bg.close_video();
        }
    }

    pub fn apply_live_json(&mut self, json_line: &str, now_ms: u64) -> Result<(), String> {
        let res = self.state.apply_live_json(json_line, now_ms);
        self.state.handle_key_press_actions(&self.cfg, now_ms);
        if self.cfg.reconcile_live_theme(&self.state) {
            self.reconcile_config_derived_state();
        }
        res
    }

    pub fn set_custom_bg_rgba(
        &mut self,
        rgba: &[u8],
        width: i32,
        height: i32,
    ) -> Result<(), String> {
        self.renderer.bg.set_custom_rgba(rgba, width, height)
    }

    pub fn clear_custom_bg(&mut self) {
        self.renderer.bg.clear_custom();
    }

    pub fn submit_video_frame_rgb24(
        &mut self,
        rgb24: &[u8],
        width: i32,
        height: i32,
        pts_ms: u64,
    ) -> Result<(), String> {
        self.renderer.bg.submit_video_frame_rgb24(
            &self.cfg.appearance.bg_path,
            rgb24,
            width,
            height,
            pts_ms,
        )
    }

    pub fn render_argb(&mut self, now_sec: i64, now_ms: u64, suspended: bool) -> Vec<u32> {
        self.state.tick_pomo(now_ms);
        self.renderer.render_frame_at(
            &self.cfg,
            &self.state,
            self.media_dir.as_deref(),
            now_sec,
            now_ms,
            suspended,
        );
        self.renderer.pixels.clone()
    }

    pub fn render_rgba(&mut self, now_sec: i64, now_ms: u64, suspended: bool) -> Vec<u8> {
        self.state.tick_pomo(now_ms);
        self.renderer.render_frame_at(
            &self.cfg,
            &self.state,
            self.media_dir.as_deref(),
            now_sec,
            now_ms,
            suspended,
        );
        let mut out = vec![0u8; self.renderer.pixels.len() * 4];
        for (dst, &argb) in out.chunks_exact_mut(4).zip(&self.renderer.pixels) {
            dst[0] = ((argb >> 16) & 0xFF) as u8;
            dst[1] = ((argb >> 8) & 0xFF) as u8;
            dst[2] = (argb & 0xFF) as u8;
            dst[3] = ((argb >> 24) & 0xFF) as u8;
        }
        out
    }

    fn encode_ppm(&self) -> Vec<u8> {
        let hdr = format!(
            "P6\n{} {}\n255\n",
            self.renderer.width, self.renderer.height
        );
        let mut out = Vec::with_capacity(hdr.len() + self.renderer.pixels.len() * 3);
        out.extend_from_slice(hdr.as_bytes());
        for &argb in &self.renderer.pixels {
            out.push(((argb >> 16) & 0xFF) as u8);
            out.push(((argb >> 8) & 0xFF) as u8);
            out.push((argb & 0xFF) as u8);
        }
        out
    }

    pub fn render_ppm_bytes(&mut self, now_sec: i64, now_ms: u64, suspended: bool) -> Vec<u8> {
        self.state.tick_pomo(now_ms);
        self.renderer.render_frame_at(
            &self.cfg,
            &self.state,
            self.media_dir.as_deref(),
            now_sec,
            now_ms,
            suspended,
        );
        self.encode_ppm()
    }

    pub fn write_ppm(&mut self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.encode_ppm()).map_err(|e| e.to_string())
    }

    pub fn reload_assets(&mut self) {
        self.renderer.assets.invalidate();
        self.renderer.bg.invalidate();
    }

    pub fn get_key_rects(&self) -> Vec<FluxKeyRect> {
        self.renderer.phys_layout.keys.clone()
    }
}
