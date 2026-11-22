//! Built-in WGSL shaders for the OpenFlux v2 7-stage GPU compositor.

pub const FS_TRIANGLE_VERT: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(3.0, 1.0),
        vec2<f32>(-1.0, 1.0)
    );
    var out: VsOut;
    let xy = p[vid];
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (1.0 - xy.y) * 0.5);
    return out;
}
"#;

/// Fallback procedural background shader when no background module or media is active.
pub const DEFAULT_BG_WGSL: &str = r#"
struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
    params: array<vec4<f32>, 4>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = u.time * 0.35;
    let w1 = sin(uv.x * 5.0 + t) * cos(uv.y * 3.0 - t * 0.7);
    let w2 = sin((uv.x * 0.7 - uv.y * 1.1) * 7.0 + t * 1.3);
    let band = smoothstep(-0.6, 0.9, w1 * 0.6 + w2 * 0.4);
    let deep = vec3<f32>(0.03, 0.05, 0.11);
    let teal = vec3<f32>(0.06, 0.24, 0.38);
    let violet = vec3<f32>(0.22, 0.10, 0.34);
    let col = mix(deep, mix(teal, violet, uv.x), band * 0.85);
    return vec4<f32>(col, 1.0);
}
"#;

/// NV12 (BT.709) hardware video plane decoder straight into `scene_bg`.
pub const NV12_TO_RGBA_WGSL: &str = r#"
@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var uv_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let y = textureSample(y_tex, samp, uv).r;
    let cbcr = textureSample(uv_tex, samp, uv).rg - vec2<f32>(0.5, 0.5);
    let y_s = (y - 0.062745) * 1.164383;
    let r = y_s + 1.792741 * cbcr.y;
    let g = y_s - 0.213249 * cbcr.x - 0.532909 * cbcr.y;
    let b = y_s + 2.112402 * cbcr.x;
    return vec4<f32>(clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// RGBA image blit straight into `scene_bg`.
pub const BLIT_RGBA_WGSL: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(src_tex, samp, uv);
}
"#;

/// 2D damped wave equation ping-pong simulation pass (`Rgba16Float`, R = h_curr, G = h_prev).
pub const WAVE_SIM_WGSL: &str = r#"
struct WaveImpulse {
    pos_rad_force: vec4<f32>, // xy in UV [0,1], z = radius in UV-X, w = force
};

struct WaveSimUniforms {
    texel: vec2<f32>,
    damping: f32,
    speed: f32,
    aspect: f32,
    impulse_count: u32,
    _pad0: u32,
    _pad1: u32,
    impulses: array<WaveImpulse, 32>,
};

@group(0) @binding(0) var prev_state: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> u: WaveSimUniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let cur = textureSample(prev_state, samp, uv);
    let h_c = cur.r;
    let h_prev = cur.g;

    let h_l = textureSample(prev_state, samp, uv + vec2<f32>(-u.texel.x, 0.0)).r;
    let h_r = textureSample(prev_state, samp, uv + vec2<f32>( u.texel.x, 0.0)).r;
    let h_u = textureSample(prev_state, samp, uv + vec2<f32>(0.0, -u.texel.y)).r;
    let h_d = textureSample(prev_state, samp, uv + vec2<f32>(0.0,  u.texel.y)).r;

    let lap = (h_l + h_r + h_u + h_d) * 0.25 - h_c;
    var h_next = (2.0 * h_c - h_prev) + lap * (u.speed * 1.9);

    // Apply incoming impulses (key presses, Idle Pet paws, CLI shockwaves).
    let count = min(u.impulse_count, 32u);
    for (var i: u32 = 0u; i < count; i = i + 1u) {
        let imp = u.impulses[i].pos_rad_force;
        let diff = vec2<f32>((uv.x - imp.x) * u.aspect, uv.y - imp.y);
        let r = max(imp.z, 0.005);
        let d2 = dot(diff, diff) / (r * r);
        if (d2 < 3.5) {
            // Smooth Mexican-hat-like splash drop for crisp expanding concentric rings.
            let drop = exp(-d2 * 2.2) * (1.0 - 0.35 * d2) * imp.w;
            h_next = h_next + drop;
        }
    }

    // Soft edge absorption so waves don't reflect harshly off display edges.
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    let edge_damp = smoothstep(0.0, 0.035, edge);
    h_next = clamp(h_next * u.damping * mix(0.88, 1.0, edge_damp), -2.5, 2.5);

    return vec4<f32>(h_next, h_c, abs(h_next), 1.0);
}
"#;

/// 3D Water Refraction & Fresnel Specular Composite pass:
/// Reads the single-crest analytical `WaveField2D` normal/height texture (`RGBA8`: R=0.5+gx*0.5,
/// G=0.5+gy*0.5, B=0.5+h*0.35) so Studio preview and the physical RK3566 keyboard match identically!
pub const REFRACT_COMPOSITE_WGSL: &str = r#"
struct RefractUniforms {
    texel: vec2<f32>,
    refraction_px: f32,
    specular_strength: f32,
    resolution: vec2<f32>,
    enabled: f32,
    _pad: f32,
    tint: vec4<f32>,
};

@group(0) @binding(0) var bg_tex: texture_2d<f32>;
@group(0) @binding(1) var wave_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: RefractUniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    if (u.enabled < 0.5) {
        return textureSample(bg_tex, samp, uv);
    }

    let w = textureSample(wave_tex, samp, uv);
    let grad = (w.rg - vec2<f32>(0.5, 0.5)) * 2.0;
    let h_c = (w.b - 0.5) / 0.35;
    let offset_uv = (grad * u.refraction_px) / u.resolution;

    // Subtle RGB chromatic dispersion along the single outgoing crest for crisp glass/water feel.
    let uv_r = clamp(uv + offset_uv * 1.12, vec2<f32>(0.001), vec2<f32>(0.999));
    let uv_g = clamp(uv + offset_uv,        vec2<f32>(0.001), vec2<f32>(0.999));
    let uv_b = clamp(uv + offset_uv * 0.88, vec2<f32>(0.001), vec2<f32>(0.999));

    let bg_r = textureSample(bg_tex, samp, uv_r).r;
    let bg_g = textureSample(bg_tex, samp, uv_g).g;
    let bg_b = textureSample(bg_tex, samp, uv_b).b;
    var col = vec3<f32>(bg_r, bg_g, bg_b);

    let n = normalize(vec3<f32>(-grad.x * 4.2, -grad.y * 4.2, 1.0));
    let light1 = normalize(vec3<f32>(-0.32, -0.52, 0.79));
    let view = vec3<f32>(0.0, 0.0, 1.0);

    // Single directional sheen + crisp crest glint right on the single wave peak (no double ring!).
    let spec = pow(max(dot(n, normalize(light1 + view)), 0.0), 26.0) * smoothstep(0.05, 0.45, max(h_c, 0.0));
    let crest_glint = pow(clamp(h_c, 0.0, 1.0), 2.2) * 0.38;

    col = col + u.tint.rgb * (spec * u.specular_strength + crest_glint * u.specular_strength);
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// GPU instanced smooth Key Glow / Ring / Ripple / Solid Highlight + Particle billboard pass.
pub const KEY_GLOW_WGSL: &str = r#"
struct GlowInstance {
    // xy = center px, zw = size (w, h) px
    rect: vec4<f32>,
    // x = shape (0=Glow, 1=Ring, 2=Ripple, 3=Solid, 4=Particle), y = radius_px, z = thickness_px, w = intensity
    params: vec4<f32>,
    color: vec4<f32>,
};

struct ScreenUniform {
    resolution: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: ScreenUniform;
@group(0) @binding(1) var<storage, read> instances: array<GlowInstance>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local_px: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) params: vec4<f32>,
    @location(3) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vid: u32,
    @builtin(instance_index) iid: u32,
) -> VsOut {
    let inst = instances[iid];
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    let c = corners[vid];
    let half_sz = inst.rect.zw * 0.5;
    let expand = inst.params.y * 2.5 + inst.params.z + 28.0;
    let extent = half_sz + vec2<f32>(expand, expand);
    let local = c * extent;
    let px = inst.rect.xy + local;
    let ndc = vec2<f32>(
        (px.x / screen.resolution.x) * 2.0 - 1.0,
        1.0 - (px.y / screen.resolution.y) * 2.0,
    );

    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local_px = local;
    out.half_size = half_sz;
    out.params = inst.params;
    out.color = inst.color;
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let shape = i32(in.params.x + 0.5);
    let radius = max(in.params.y, 1.0);
    let thick = max(in.params.z, 1.0);
    let intensity = in.params.w;

    var alpha = 0.0;
    var boost = 1.0;

    if (shape == 0) {
        // Soft rounded-box under-key glow (used by Analog Depth + Velocity Glow!)
        let d = sd_round_box(in.local_px, in.half_size, 10.0);
        let inside = smoothstep(4.0, -6.0, d);
        let dist_out = max(d, 0.0);
        let fade_window = smoothstep(radius * 2.3, radius * 0.4, dist_out);
        let halo = exp(-dist_out / max(radius * 0.48, 4.0)) * fade_window;
        alpha = clamp(inside * 0.45 + halo * 0.78, 0.0, 1.0) * intensity;
        boost = 1.0 + inside * 0.35;
    } else if (shape == 1) {
        // Expanding rounded-box ring around keycap
        let d = sd_round_box(in.local_px, in.half_size + vec2<f32>(radius, radius), 12.0);
        let band = smoothstep(thick, 0.0, abs(d));
        alpha = band * intensity;
        boost = 1.2;
    } else if (shape == 2) {
        // Expanding circular 2D ripple
        let dist = length(in.local_px);
        let band = smoothstep(thick, 0.0, abs(dist - radius));
        alpha = band * intensity;
        boost = 1.15;
    } else if (shape == 3) {
        // Crisp solid highlight fill
        let d = sd_round_box(in.local_px, in.half_size, 10.0);
        alpha = smoothstep(1.5, -1.5, d) * intensity;
    } else {
        // Glowing optical particle (core + soft aura; magnified when passing under keycap lens!)
        let dist = length(in.local_px);
        let core = smoothstep(radius, radius * 0.25, dist);
        let aura = exp(-dist / max(radius * 1.35, 2.0)) * 0.55;
        alpha = clamp(core + aura, 0.0, 1.0) * intensity;
        boost = 1.0 + core * 0.6;
    }

    let final_a = clamp(alpha * in.color.a, 0.0, 1.0);
    if (final_a < 0.002) {
        discard;
    }
    return vec4<f32>(clamp(in.color.rgb * boost, vec3<f32>(0.0), vec3<f32>(1.0)), final_a);
}
"#;
