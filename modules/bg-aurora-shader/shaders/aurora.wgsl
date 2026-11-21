struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
    params: array<vec4<f32>, 4>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let speed = max(u.params[0].x, 0.1);
    let intensity = max(u.params[0].y, 0.2);
    let t = u.time * 0.45 * speed;

    // Flowing aurora ribbons + crisp diagonal caustic structure so 3D water refraction pops clearly!
    let p = vec2<f32>(uv.x * 1.777, uv.y);
    let r1 = sin(p.x * 3.8 + t + sin(p.y * 4.2 - t * 0.6) * 0.9);
    let r2 = sin(p.x * 6.5 - p.y * 3.2 - t * 1.15);
    let r3 = cos((p.x + p.y) * 9.0 + t * 0.8);

    let ribbon1 = smoothstep(-0.35, 0.95, r1 * 0.65 + r2 * 0.35);
    let ribbon2 = smoothstep(0.1, 0.98, r2 * 0.55 + r3 * 0.45);

    let deep = vec3<f32>(0.025, 0.04, 0.09);
    let cyan = vec3<f32>(0.06, 0.42, 0.58);
    let emerald = vec3<f32>(0.07, 0.52, 0.44);
    let violet = vec3<f32>(0.36, 0.14, 0.56);

    var col = mix(deep, mix(cyan, emerald, uv.y), ribbon1 * 0.72);
    col = mix(col, violet, ribbon2 * 0.42 * (0.35 + 0.65 * uv.x));

    // Subtle geometric constellation lines that bend dramatically under water ripples
    let grid_x = smoothstep(0.96, 1.0, abs(sin(uv.x * 34.0 + r1 * 0.35)));
    let grid_y = smoothstep(0.96, 1.0, abs(sin(uv.y * 20.0 + r2 * 0.35)));
    col = col + vec3<f32>(0.16, 0.34, 0.48) * max(grid_x, grid_y) * 0.22;

    return vec4<f32>(clamp(col * intensity, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
