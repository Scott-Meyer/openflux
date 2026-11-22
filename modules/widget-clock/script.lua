-- Exact v1 Digital Clock Tile (`TileType::DigitalClock`):
-- Large centered 72px light time + subtle AM/PM + uppercase full date line.

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end

  local tx, ty, tw, th = sr.x, sr.y, sr.w, sr.h
  local use_24h = module.config().use_24h == true
  local now = os.date("*t")
  local white = { 1.0, 1.0, 1.0, 0.98 }
  local subtle = { 0.74, 0.80, 0.88, 0.78 }
  local glass_bg = { 0.03, 0.04, 0.06, 0.56 }

  ctx:clip(tx, ty, tw, th)

  local cx = tx + tw * 0.5
  local y_time = ty + th * 0.5 - 52

  if use_24h then
    local time_s = string.format("%02d:%02d", now.hour, now.min)
    ctx:text(cx, y_time, time_s, 72, white, "center")
  else
    local h12 = now.hour % 12
    if h12 == 0 then h12 = 12 end
    local time_s = string.format("%d:%02d", h12, now.min)
    local ampm = (now.hour >= 12) and "PM" or "AM"
    -- Center the combined time + AM/PM nicely
    local shift = (#time_s >= 5) and 22 or 18
    ctx:text(cx - shift, y_time, time_s, 72, white, "center")
    local ampm_x = cx + (#time_s >= 5 and 76 or 58)
    ctx:text(ampm_x, y_time + 40, ampm, 24, subtle, "left")
  end

  local date_s = string.upper(os.date("%A, %b %d, %Y"))
  ctx:text(cx, ty + th - 44, date_s, 15, subtle, "center")
  ctx:clear_clip()
end
