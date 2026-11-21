function on_render(ctx, stage)
  if stage ~= "background" then return end
  local cfg = module.config()
  local speed = tonumber(cfg.speed) or 1.0
  local intensity = tonumber(cfg.intensity) or 1.0
  ctx:shader("shaders/aurora.wgsl", { speed, intensity, 0.0, 0.0 })
end
