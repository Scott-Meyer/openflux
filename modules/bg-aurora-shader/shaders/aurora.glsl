// Universal OpenFlux GLSL Shader (`bg-aurora-shader`)
// Runs identically on BOTH Mac/Windows/Linux Studio (wgpu Metal/Vulkan/DX12)
// AND the physical RK3566 keyboard (Mali-G52 GLES2)!

vec4 effect_main(vec2 uv) {
  float speed = (u_params[0].x > 0.01) ? u_params[0].x : 1.0;
  float intensity = (u_params[0].y > 0.01) ? u_params[0].y : 1.0;
  float t = u_time * 0.45 * speed;

  vec2 p = vec2(uv.x * 1.7777, uv.y);
  float r1 = sin(p.x * 3.8 + t + sin(p.y * 4.2 - t * 0.6) * 0.9);
  float r2 = sin(p.x * 6.5 - p.y * 3.2 - t * 1.15);
  float r3 = cos((p.x + p.y) * 9.0 + t * 0.8);

  float rib1 = smoothstep(-0.35, 0.95, r1 * 0.65 + r2 * 0.35);
  float rib2 = smoothstep(0.10, 0.98, r2 * 0.55 + r3 * 0.45);

  vec3 deep = vec3(0.025, 0.04, 0.09);
  vec3 cyan = vec3(0.06, 0.42, 0.58);
  vec3 emerald = vec3(0.07, 0.52, 0.44);
  vec3 violet = vec3(0.36, 0.14, 0.56);

  vec3 col = mix(deep, mix(cyan, emerald, uv.y), rib1 * 0.72);
  col = mix(col, violet, rib2 * 0.42 * (0.35 + 0.65 * uv.x));

  float gx = smoothstep(0.96, 1.0, abs(sin(uv.x * 34.0 + r1 * 0.35)));
  float gy = smoothstep(0.96, 1.0, abs(sin(uv.y * 20.0 + r2 * 0.35)));
  col += vec3(0.16, 0.34, 0.48) * max(gx, gy) * 0.22;

  return vec4(clamp(col * intensity, vec3(0.0), vec3(1.0)), 1.0);
}
