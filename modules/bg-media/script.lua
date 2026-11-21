function on_render(ctx, stage)
  if stage ~= "background" then return end
  local p = module.config().media_path
  if type(p) == "string" and #p > 0 then
    ctx:background_media(p)
  end
end
