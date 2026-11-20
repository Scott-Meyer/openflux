use crate::config::{KeyAction, SettingsConfig, StylemapLayer};

pub const DISPLAY_WIDTH: i32 = 1920;
pub const DISPLAY_HEIGHT: i32 = 1080;
pub const MAX_KEYS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLayoutType {
    Ansi,
    Iso,
}

#[derive(Debug, Clone, Copy)]
pub struct FluxKeyRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub key_index: u16,
}

#[derive(Debug, Clone)]
pub struct PhysicalLayout {
    pub layout: FrameLayoutType,
    pub keys: Vec<FluxKeyRect>,
    pub mosaic_x: f32,
    pub mosaic_y: f32,
    pub mosaic_w: f32,
    pub mosaic_h: f32,
    pub mosaic_gap: f32,
}

fn add_row_mm(
    keys: &mut Vec<FluxKeyRect>,
    widths_mm: &[f32],
    y_px: f32,
    key_h_px: f32,
    gap_mm: f32,
    frame_w_mm: f32,
    px_per_mm: f32,
) {
    let n = widths_mm.len();
    let row_w_mm: f32 =
        widths_mm.iter().sum::<f32>() + if n > 0 { (n - 1) as f32 * gap_mm } else { 0.0 };
    let mut x_mm = (frame_w_mm - row_w_mm) * 0.5;
    for &w_mm in widths_mm {
        if w_mm > 0.0 && keys.len() < MAX_KEYS {
            let idx = keys.len() as u16;
            keys.push(FluxKeyRect {
                x: x_mm * px_per_mm,
                y: y_px,
                w: w_mm * px_per_mm,
                h: key_h_px,
                key_index: idx,
            });
            x_mm += w_mm + gap_mm;
        }
    }
}

pub fn build_physical_layout(layout: FrameLayoutType) -> PhysicalLayout {
    // Physical keyboard frame dimensions: 309.31 mm active LCD width (1920 px),
    // 17.0 mm key height, 2.0 mm standard gap, 6.75 mm F-row gap, 3.07 mm bottom margin.
    let frame_w_mm = 309.31_f32;
    let px_per_mm = 1920.0_f32 / frame_w_mm;
    let key_h_mm = 17.0_f32;
    let gap_mm = 2.0_f32;
    let bottom_mm = 3.07_f32;
    let key_h_px = key_h_mm * px_per_mm;
    let row_gaps_below_mm = [6.75_f32, 2.0, 2.0, 2.0, 2.0, bottom_mm];

    let mut row_y_px = [0.0_f32; 6];
    let mut y_bottom_mm = 1080.0_f32 / px_per_mm;
    for r in (0..6).rev() {
        y_bottom_mm -= row_gaps_below_mm[r];
        let y_top_mm = y_bottom_mm - key_h_mm;
        row_y_px[r] = y_top_mm * px_per_mm;
        y_bottom_mm = y_top_mm;
    }

    // Mosaic header bar spans the top band above the keyframe rows.
    let mosaic_margin_x = 8.1_f32;
    let mosaic_x = mosaic_margin_x;
    let mosaic_w = 1920.0_f32 - 2.0 * mosaic_margin_x;
    let mosaic_y = 5.0_f32;
    let total_kb_h_px = (1080.0_f32 - (y_bottom_mm * px_per_mm)).max(600.0);
    let avail_header_h = (1080.0_f32 - total_kb_h_px).max(200.0);
    let mosaic_h = (avail_header_h - 80.0_f32).max(160.0);
    let mosaic_gap = 89.4_f32;

    let mut keys = Vec::with_capacity(88);
    let row0 = [17.0_f32; 16];
    add_row_mm(
        &mut keys,
        &row0,
        row_y_px[0],
        key_h_px,
        gap_mm,
        frame_w_mm,
        px_per_mm,
    );

    let row1 = [
        17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 36.0, 17.0,
    ];
    add_row_mm(
        &mut keys,
        &row1,
        row_y_px[1],
        key_h_px,
        gap_mm,
        frame_w_mm,
        px_per_mm,
    );

    if layout == FrameLayoutType::Iso {
        let row2 = [
            26.53, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 26.53,
            17.0,
        ];
        let row3 = [
            31.29, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 21.77,
            17.0,
        ];
        let row4 = [
            21.77, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 31.29, 17.0,
            17.0,
        ];
        let row5 = [
            21.77, 17.0, 17.0, 17.0, 107.49, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
        ];
        add_row_mm(
            &mut keys,
            &row2,
            row_y_px[2],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
        add_row_mm(
            &mut keys,
            &row3,
            row_y_px[3],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
        add_row_mm(
            &mut keys,
            &row4,
            row_y_px[4],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
        add_row_mm(
            &mut keys,
            &row5,
            row_y_px[5],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
    } else {
        let row2 = [
            26.53, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 26.53,
            17.0,
        ];
        let row3 = [
            31.29, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 40.82, 17.0,
        ];
        let row4 = [
            40.82, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0, 31.29, 17.0, 17.0,
        ];
        let row5 = [
            21.77, 17.0, 17.0, 17.0, 107.49, 17.0, 17.0, 17.0, 17.0, 17.0, 17.0,
        ];
        add_row_mm(
            &mut keys,
            &row2,
            row_y_px[2],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
        add_row_mm(
            &mut keys,
            &row3,
            row_y_px[3],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
        add_row_mm(
            &mut keys,
            &row4,
            row_y_px[4],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
        add_row_mm(
            &mut keys,
            &row5,
            row_y_px[5],
            key_h_px,
            gap_mm,
            frame_w_mm,
            px_per_mm,
        );
    }

    PhysicalLayout {
        layout,
        keys,
        mosaic_x,
        mosaic_y,
        mosaic_w,
        mosaic_h,
        mosaic_gap,
    }
}

pub fn compute_key_parallax(
    settings: &SettingsConfig,
    key_cx_px: f64,
    key_cy_px: f64,
) -> (f64, f64) {
    // Snell's-law optical refraction through 9.0 mm acrylic keycaps (refractive index n = 1.51)
    // over a 307.0 x 123.0 mm keycap array aligned to the 309.31 x 121.82 model space.
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
    // Map screen y to distance from the keyboard front edge in model units (`[0 .. FRAME_H_UNITS]`).
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

pub fn keycode_to_legend(kc: &str, action_type: u8, target_layer: u8) -> String {
    if action_type == 2 {
        return if target_layer == 1 {
            "FN".to_string()
        } else {
            format!("L{target_layer}")
        };
    }
    if action_type == 1 {
        return format!("M{target_layer}");
    }
    if kc.is_empty() || kc == "KC_TRANSPARENT" || kc == "KC_NO" {
        return String::new();
    }
    let s = kc.strip_prefix("KC_").unwrap_or(kc);
    match s {
        "ESCAPE" => "Esc",
        "BACKSPACE" => "Backspace",
        "DELETE" => "Delete",
        "CAPS_LOCK" => "Caps",
        "LEFT_SHIFT" | "RIGHT_SHIFT" => "Shift",
        "LEFT_CTRL" | "RIGHT_CTRL" => "Ctrl",
        "LEFT_ALT" | "RIGHT_ALT" => "Opt",
        "LEFT_GUI" | "RIGHT_GUI" => "Cmd",
        "ENTER" => "Enter",
        "SPACE" => "Space",
        "TAB" => "Tab",
        "GRAVE" => "`",
        "MINUS" => "-",
        "EQUAL" => "=",
        "LEFT_BRACKET" => "[",
        "RIGHT_BRACKET" => "]",
        "BACKSLASH" | "NONUS_HASH" => "\\",
        "SEMICOLON" => ";",
        "QUOTE" => "'",
        "COMMA" => ",",
        "DOT" => ".",
        "SLASH" => "/",
        "PAGE_UP" => "PgUp",
        "PAGE_DOWN" => "PgDn",
        "HOME" => "Home",
        "END" => "End",
        "INSERT" => "Ins",
        "PSCREEN" | "PRINT_SCREEN" => "PrtSc",
        "GLOBE" => "Fn",
        "UP" => "^",
        "DOWN" => "v",
        "LEFT" => "<",
        "RIGHT" => ">",
        "AUDIO_VOL_UP" => "Vol+",
        "AUDIO_VOL_DOWN" => "Vol-",
        "AUDIO_MUTE" => "Mute",
        "MEDIA_PLAY_PAUSE" => "Play",
        "MEDIA_NEXT_TRACK" => "Next",
        "MEDIA_PREV_TRACK" => "Prev",
        "BRIGHTNESS_UP" => "Bri+",
        "BRIGHTNESS_DOWN" => "Bri-",
        "OF_START_PAUSE_POMODORO" => "Pomo",
        "OF_RESET_POMODORO" => "Rst",
        _ => return s.chars().take(6).collect(),
    }
    .to_string()
}

pub fn resolve_key_style<'a>(
    actions: &[KeyAction],
    fallback_actions: &[KeyAction],
    style_layer: Option<&'a StylemapLayer>,
    default_legend_argb: u32,
    default_ring_argb: u32,
    key_index: usize,
) -> (String, Option<&'a str>, u32, u32) {
    let mut leg = actions
        .get(key_index)
        .filter(|a| !a.legend.is_empty())
        .or_else(|| {
            fallback_actions
                .get(key_index)
                .filter(|a| !a.legend.is_empty())
        })
        .map(|a| a.legend.clone())
        .unwrap_or_default();
    let mut leg_col = default_legend_argb;
    let mut ring_col = default_ring_argb;
    let mut icon: Option<&'a str> = None;

    if let Some(sl) = style_layer {
        if let Some(ref ds) = sl.default_style {
            if let Some(c) = ds.legend_argb {
                leg_col = c;
            }
            if let Some(c) = ds.ring_argb {
                ring_col = c;
            }
        }
        if let Some(ko) = sl.keys.get(&key_index) {
            if let Some(ref l) = ko.label {
                leg = l.clone();
            }
            if let Some(c) = ko.legend_argb {
                leg_col = c;
            }
            if let Some(c) = ko.ring_argb {
                ring_col = c;
            }
            if let Some(ref ic) = ko.icon {
                if !ic.is_empty() {
                    icon = Some(ic.as_str());
                }
            }
        }
    }
    (leg, icon, leg_col, ring_col)
}
