//! Declarative Frame Contribution IR emitted by modules for the GPU compositor.
//!
//! Keeping this IR independent of `wgpu` allows the module runtime to remain pure, testable,
//! and backend-agnostic while giving every module full visual expressiveness across all stages.

use crate::manifest::CompositorStage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DrawCommand {
    Clip {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    ClearClip,
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    RoundRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: [f32; 4],
        #[serde(default)]
        stroke_width: f32,
        #[serde(default)]
        stroke_color: [f32; 4],
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        color: [f32; 4],
        #[serde(default)]
        stroke_width: f32,
        #[serde(default)]
        stroke_color: [f32; 4],
    },
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        color: [f32; 4],
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: [f32; 4],
    },
    Arc {
        cx: f32,
        cy: f32,
        r: f32,
        start_deg: f32,
        end_deg: f32,
        width: f32,
        color: [f32; 4],
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: [f32; 4],
        #[serde(default)]
        align: TextAlign,
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        /// Relative to the module directory or an absolute path.
        path: String,
        #[serde(default = "default_white")]
        tint: [f32; 4],
    },
}

fn default_white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

/// GPU 2D wave heightfield simulation configuration + impulses submitted by distortion modules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveImpulseCmd {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub force: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveFieldParams {
    pub damping: f32,
    pub speed: f32,
    pub refraction_strength: f32,
    pub specular_strength: f32,
    pub tint: [f32; 4],
}

impl Default for WaveFieldParams {
    fn default() -> Self {
        Self {
            damping: 0.986,
            speed: 0.42,
            refraction_strength: 28.0,
            specular_strength: 0.55,
            tint: [0.65, 0.88, 1.0, 1.0],
        }
    }
}

/// Smooth GPU key-glow / ring primitive for under-key effects (analog depth glow, classic effects).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyGlowShape {
    /// Soft radial/rounded-box glow (ideal for analog depth + velocity glow and classic glow).
    Glow,
    /// Expanding crisp outline ring around a keycap.
    Ring,
    /// Expanding radial ripple circle.
    Ripple,
    /// Solid rounded keycap highlight.
    Solid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyGlowCmd {
    pub shape: KeyGlowShape,
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// Spread/ring radius in pixels.
    pub radius_px: f32,
    /// Ring thickness in pixels (for `Ring` / `Ripple`).
    pub thickness_px: f32,
    /// Intensity multiplier `[0.0..2.0]`.
    pub intensity: f32,
    pub color: [f32; 4],
}

/// Particle spawn request handled by the compositor's key-SDF physics & refraction engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticleSpawnCmd {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub radius_px: f32,
    pub life_ms: f32,
    pub color: [f32; 4],
    /// Probability `[0.0, 1.0]` that hitting a keycap wall bounces in the 2mm trench vs enters under the lens.
    pub bounce_chance: f32,
    /// Lens magnification/refraction multiplier when passing underneath a transparent keycap.
    pub lens_refract: f32,
    pub drag: f32,
}

/// Custom WGSL or GLSL fragment shader invocation for a compositor stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShaderPassCmd {
    pub module_id: String,
    pub stage: CompositorStage,
    /// Absolute filesystem path to the `.wgsl` or `.frag`/`.glsl` shader file.
    pub shader_path: String,
    /// Up to 4 `vec4<f32>` user uniforms passed as `u_params[0..4]`.
    pub params: [[f32; 4]; 4],
}

/// Aggregated visual output from all active modules for one compositor frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameScene {
    pub time_sec: f32,
    pub dt_sec: f32,
    pub background_shaders: Vec<ShaderPassCmd>,
    pub background_media: Option<String>,
    pub distortion_enabled: bool,
    pub wave_params: WaveFieldParams,
    pub wave_impulses: Vec<WaveImpulseCmd>,
    pub distortion_shaders: Vec<ShaderPassCmd>,
    pub under_key_shaders: Vec<ShaderPassCmd>,
    pub key_glows: Vec<KeyGlowCmd>,
    pub particle_spawns: Vec<ParticleSpawnCmd>,
    pub under_key_draws: Vec<DrawCommand>,
    pub tile_draws: Vec<DrawCommand>,
    pub world_overlay_draws: Vec<DrawCommand>,
}
