function on_render(ctx, stage)
  if stage ~= "background" then return end
  local cfg = module.config()
  local p = cfg.media_path
  if type(p) == "string" and #p > 0 then
    ctx:background_media(p)
  end
  local dim = tonumber(cfg.bg_dimming) or 0
  if dim > 0 then
    ctx:background_dimming(dim / 100.0)
  end
end
