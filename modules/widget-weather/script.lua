local weather = { city = "Local", temp = 68, high = 72, low = 55, condition = "Clear", celsius = false, ready = false }

function on_event(topic, payload)
  if topic == "host/weather/current" and type(payload) == "table" then
    weather = payload
  end
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end
  local unit = weather.celsius and "C" or "F"

  ctx:clip(sr.x, sr.y, sr.w, sr.h)
  ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.72 }, 1.5, { 0.4, 0.76, 1.0, 0.45 })
  ctx:text(sr.x + 22, sr.y + 16, tostring(weather.city or "Weather"), 20, { 0.72, 0.88, 1.0, 0.92 }, "left")
  ctx:text(sr.x + 22, sr.y + 46, string.format("%.0f%s", tonumber(weather.temp) or 0, unit), 50, { 1.0, 1.0, 1.0, 0.98 }, "left")
  ctx:text(sr.x + 22, sr.y + 110, tostring(weather.condition or ""), 20, { 0.8, 0.88, 0.96, 0.86 }, "left")
  ctx:text(sr.x + sr.w - 22, sr.y + 62, string.format("H:%.0f L:%.0f", tonumber(weather.high) or 0, tonumber(weather.low) or 0), 18, { 0.65, 0.8, 0.95, 0.85 }, "right")
  ctx:clear_clip()
end
