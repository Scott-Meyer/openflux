-- Responsive Digital Clock Widget (`widget-clock`)
-- Supports 0.5x (compact time + day), 1x (classic v1 time + full date), and 2x (widescreen time + seconds + date).

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end

  local tx, ty, tw, th = sr.x, sr.y, sr.w, sr.h
  local use_24h = module.config().use_24h == true
  local now = os.date("*t")
  local white = { 1.0, 1.0, 1.0, 0.98 }
  local subtle = { 0.74, 0.80, 0.88, 0.78 }
  local cx = tx + tw * 0.5

  ctx:clip(tx, ty, tw, th)

  if tw < 260 then
    -- Half-slot compact layout
    local h_val = use_24h and now.hour or ((now.hour % 12 == 0) and 12 or (now.hour % 12))
    local time_s = use_24h and string.format("%02d:%02d", h_val, now.min) or string.format("%d:%02d", h_val, now.min)
    ctx:text(cx, ty + 30, time_s, 48, white, "center")
    local sub = use_24h and string.upper(os.date("%a %d")) or (string.upper(os.date("%a %d")) .. "  " .. (now.hour >= 12 and "PM" or "AM"))
    ctx:text(cx, ty + 98, sub, 15, subtle, "center")
  elseif tw > 580 then
    -- Widescreen 2-slot layout: large time + live seconds on left, full weekday + date on right
    local h_val = use_24h and now.hour or ((now.hour % 12 == 0) and 12 or (now.hour % 12))
    local time_s = use_24h and string.format("%02d:%02d", h_val, now.min) or string.format("%d:%02d", h_val, now.min)
    local sec_s = string.format(":%02d", now.sec)
    local left_x = tx + tw * 0.34
    ctx:text(left_x, ty + th * 0.5 - 46, time_s, 84, white, "center")
    ctx:text(left_x + (#time_s >= 5 and 118 or 98), ty + th * 0.5 - 12, sec_s, 30, subtle, "left")
    if not use_24h then
      ctx:text(left_x + (#time_s >= 5 and 118 or 98), ty + th * 0.5 - 44, (now.hour >= 12) and "PM" or "AM", 22, subtle, "left")
    end
    ctx:rect(tx + tw * 0.60, ty + 28, 1.0, th - 56, { 1, 1, 1, 0.14 })
    local rx = tx + tw * 0.64
    ctx:text(rx, ty + 42, string.upper(os.date("%A")), 26, white, "left")
    ctx:text(rx, ty + 82, string.upper(os.date("%B %d, %Y")), 18, subtle, "left")
  else
    -- Classic 1-slot v1 layout
    local y_time = ty + th * 0.5 - 52
    if use_24h then
      local time_s = string.format("%02d:%02d", now.hour, now.min)
      ctx:text(cx, y_time, time_s, 72, white, "center")
    else
      local h12 = now.hour % 12
      if h12 == 0 then h12 = 12 end
      local time_s = string.format("%d:%02d", h12, now.min)
      local ampm = (now.hour >= 12) and "PM" or "AM"
      local shift = (#time_s >= 5) and 22 or 18
      ctx:text(cx - shift, y_time, time_s, 72, white, "center")
      local ampm_x = cx + (#time_s >= 5 and 76 or 58)
      ctx:text(ampm_x, y_time + 40, ampm, 24, subtle, "left")
    end
    local date_s = string.upper(os.date("%A, %b %d, %Y"))
    ctx:text(cx, ty + th - 44, date_s, 15, subtle, "center")
  end

  ctx:clear_clip()
end
