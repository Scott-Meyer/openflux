function on_render(ctx, stage)
  local cfg = module.config()
  if stage == "background" then
    local p = cfg.media_path
    if type(p) == "string" and #p > 0 then
      ctx:background_media(p)
    end
  elseif stage == "under_key" then
    local dim = tonumber(cfg.bg_dimming) or 0
    if dim > 1 then
      local a = math.min(0.85, (dim / 100.0) * 0.88)
      ctx:rect(0, 0, 1920, 1080, { 0.02, 0.025, 0.04, a })
    end
  end
end
