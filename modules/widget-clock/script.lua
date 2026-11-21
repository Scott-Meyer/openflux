function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end
  local accent = module.color("accent", "#4de1ff")
  local now = os.date("*t")
  local time_str = string.format("%02d:%02d", now.hour, now.min)
  local date_str = os.date("%a, %b %d")
  local sec_angle = -90.0 + (now.sec / 60.0) * 360.0

  ctx:clip(sr.x, sr.y, sr.w, sr.h)
  ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.72 }, 1.5, { accent[1], accent[2], accent[3], 0.45 })
  ctx:text(sr.x + 26, sr.y + 26, time_str, 58, { 1.0, 1.0, 1.0, 0.98 }, "left")
  ctx:text(sr.x + 28, sr.y + 102, date_str, 22, { 0.72, 0.84, 0.96, 0.88 }, "left")

  local rx = sr.x + sr.w - 64
  local ry = sr.y + sr.h * 0.5
  ctx:circle(rx, ry, 34, { 0.1, 0.14, 0.22, 0.65 }, 4.0, { 0.3, 0.4, 0.55, 0.35 })
  ctx:arc(rx, ry, 34, -90.0, sec_angle, 5.0, accent)
  ctx:text(rx, ry - 12, string.format("%02d", now.sec), 22, { 0.92, 0.96, 1.0, 0.95 }, "center")
  ctx:clear_clip()
end
