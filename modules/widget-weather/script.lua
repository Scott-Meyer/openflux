-- Exact v1 Weather Tile (`TileType::Weather`):
-- Left stats column (City, local time, Sunrise, Sunset, AQI), subtle 1px vertical hairline divider,
-- right readout (large temp, High/Low, weather icon, and 3-hour forecast strip).

local weather = {
  city = "Denver",
  temp = 0,
  high = 0,
  low = 0,
  condition = "Clear",
  is_day = true,
  celsius = false,
  ready = false,
  aqi = nil,
  sunrise = "--",
  sunset = "--",
  utc_offset_sec = nil,
  hourly = {},
}

function on_init()
  local cur = bus.get("host/weather/current")
  if type(cur) == "table" then weather = cur end
end

function on_event(topic, payload)
  if topic == "host/weather/current" and type(payload) == "table" then
    weather = payload
  end
end

local function draw_weather_icon(ctx, cx, cy, sz, cond, is_day)
  local c = string.lower(tostring(cond or ""))
  local sun_col = { 1.0, 0.84, 0.36, 0.96 }
  local moon_col = { 0.78, 0.86, 1.0, 0.95 }
  local cloud_col = { 0.88, 0.93, 0.99, 0.94 }
  local rain_col = { 0.42, 0.82, 1.0, 0.95 }

  if string.find(c, "rain") or string.find(c, "drizzle") or string.find(c, "shower") or string.find(c, "storm") then
    ctx:circle(cx - sz * 0.16, cy - sz * 0.08, sz * 0.22, cloud_col)
    ctx:circle(cx + sz * 0.08, cy - sz * 0.14, sz * 0.25, cloud_col)
    ctx:round_rect(cx - sz * 0.34, cy - sz * 0.08, sz * 0.68, sz * 0.26, sz * 0.13, cloud_col, 0, { 0, 0, 0, 0 })
    ctx:line(cx - sz * 0.16, cy + sz * 0.24, cx - sz * 0.22, cy + sz * 0.40, 2.5, rain_col)
    ctx:line(cx, cy + sz * 0.24, cx - sz * 0.06, cy + sz * 0.40, 2.5, rain_col)
    ctx:line(cx + sz * 0.16, cy + sz * 0.24, cx + sz * 0.10, cy + sz * 0.40, 2.5, rain_col)
  elseif string.find(c, "snow") or string.find(c, "sleet") then
    ctx:circle(cx - sz * 0.14, cy - sz * 0.10, sz * 0.22, cloud_col)
    ctx:circle(cx + sz * 0.10, cy - sz * 0.14, sz * 0.24, cloud_col)
    ctx:round_rect(cx - sz * 0.32, cy - sz * 0.08, sz * 0.64, sz * 0.24, sz * 0.12, cloud_col, 0, { 0, 0, 0, 0 })
    ctx:circle(cx - sz * 0.15, cy + sz * 0.30, 3.0, { 1, 1, 1, 0.95 })
    ctx:circle(cx + sz * 0.02, cy + sz * 0.34, 3.0, { 1, 1, 1, 0.95 })
    ctx:circle(cx + sz * 0.18, cy + sz * 0.29, 3.0, { 1, 1, 1, 0.95 })
  elseif string.find(c, "partly") or string.find(c, "mostly") then
    if is_day ~= false then
      ctx:circle(cx + sz * 0.14, cy - sz * 0.14, sz * 0.22, sun_col)
    else
      ctx:circle(cx + sz * 0.14, cy - sz * 0.14, sz * 0.20, moon_col)
    end
    ctx:circle(cx - sz * 0.14, cy + sz * 0.02, sz * 0.20, cloud_col)
    ctx:circle(cx + sz * 0.06, cy - sz * 0.02, sz * 0.22, cloud_col)
    ctx:round_rect(cx - sz * 0.32, cy + sz * 0.02, sz * 0.60, sz * 0.22, sz * 0.11, cloud_col, 0, { 0, 0, 0, 0 })
  elseif string.find(c, "cloud") or string.find(c, "overcast") or string.find(c, "fog") then
    ctx:circle(cx - sz * 0.16, cy - sz * 0.02, sz * 0.22, cloud_col)
    ctx:circle(cx + sz * 0.08, cy - sz * 0.08, sz * 0.26, cloud_col)
    ctx:round_rect(cx - sz * 0.34, cy - sz * 0.02, sz * 0.68, sz * 0.26, sz * 0.13, cloud_col, 0, { 0, 0, 0, 0 })
  else
    if is_day ~= false then
      ctx:circle(cx, cy, sz * 0.24, sun_col)
      for a = 0, 315, 45 do
        local rad = math.rad(a)
        ctx:line(
          cx + math.cos(rad) * sz * 0.32,
          cy + math.sin(rad) * sz * 0.32,
          cx + math.cos(rad) * sz * 0.43,
          cy + math.sin(rad) * sz * 0.43,
          2.4,
          sun_col
        )
      end
    else
      ctx:circle(cx, cy, sz * 0.26, moon_col)
      ctx:circle(cx + sz * 0.10, cy - sz * 0.08, sz * 0.22, { 0.04, 0.06, 0.10, 0.95 })
    end
  end
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end

  local tx, ty, tw, th = sr.x, sr.y, sr.w, sr.h
  local white = { 1.0, 1.0, 1.0, 0.98 }
  local subtle = { 0.72, 0.78, 0.86, 0.78 }
  local glass_bg = { 0.03, 0.04, 0.06, 0.56 }

  ctx:clip(tx, ty, tw, th)

  local cfg_city = tostring(module.config().city or "")
  local city = (#cfg_city > 0) and cfg_city or tostring(weather.city or "Denver")
  local temp_s = weather.ready and string.format("%.0f°", tonumber(weather.temp) or 0) or "--°"
  local hl_s = weather.ready and string.format("%.0f° / %.0f°", tonumber(weather.high) or 0, tonumber(weather.low) or 0) or "Syncing..."

  if tw < 260 then
    -- Half-slot compact weather tile
    local cx = tx + tw * 0.5
    draw_weather_icon(ctx, cx, ty + 42, 44, weather.condition, weather.is_day)
    ctx:text(cx, ty + 74, temp_s, 40, white, "center")
    ctx:text(cx, ty + 122, city, 14, subtle, "center")
    ctx:clear_clip()
    return
  end

  if #city > 14 then city = string.sub(city, 1, 13) .. "..." end

  local now = os.date("*t")
  local h12 = now.hour % 12
  if h12 == 0 then h12 = 12 end
  local tline = string.format("%d:%02d %s", h12, now.min, now.hour >= 12 and "PM" or "AM")

  local left_right = tx + 148
  local div_x = tx + 158
  ctx:text(tx + 12, ty + 14, city, 22, white, "left")
  ctx:text(tx + 12, ty + 42, tline, 14, subtle, "left")

  local sr_s = (type(weather.sunrise) == "string" and #weather.sunrise > 0) and weather.sunrise or "--"
  local ss_s = (type(weather.sunset) == "string" and #weather.sunset > 0) and weather.sunset or "--"
  local aqi_s = weather.aqi and tostring(math.floor(tonumber(weather.aqi) or 0)) or "--"

  local stat_y0 = ty + 84
  ctx:text(tx + 12, stat_y0, "Sunrise", 13, subtle, "left")
  ctx:text(left_right, stat_y0, sr_s, 13, white, "right")
  ctx:text(tx + 12, stat_y0 + 22, "Sunset", 13, subtle, "left")
  ctx:text(left_right, stat_y0 + 22, ss_s, 13, white, "right")
  ctx:text(tx + 12, stat_y0 + 44, "AQI", 13, subtle, "left")
  ctx:text(left_right, stat_y0 + 44, aqi_s, 13, white, "right")

  ctx:rect(div_x, ty + 16, 1, th - 36, { 1.0, 1.0, 1.0, 0.14 })

  local rx = div_x + 16
  local rw = (tx + tw - 12) - rx
  local temp_1 = weather.ready and string.format("%.1f°", tonumber(weather.temp) or 0) or "--°"

  if tw > 580 then
    -- Widescreen 2-slot layout: current temp + condition + icon on center-left, hourly forecast columns on right
    ctx:text(rx, ty + 18, temp_1, 56, white, "left")
    ctx:text(rx, ty + 84, tostring(weather.condition or ""), 20, white, "left")
    ctx:text(rx, ty + 112, "H/L " .. hl_s, 16, subtle, "left")
    draw_weather_icon(ctx, rx + 215, ty + 52, 68, weather.condition, weather.is_day)

    local fx = rx + 290
    local fw = math.max(120, (tx + tw - 16) - fx)
    ctx:rect(fx - 16, ty + 20, 1, th - 40, { 1.0, 1.0, 1.0, 0.14 })
    if weather.ready and type(weather.hourly) == "table" and #weather.hourly > 0 then
      local n = math.min(5, #weather.hourly)
      local col_w = fw / math.max(1, n)
      for i = 1, n do
        local hf = weather.hourly[i]
        local fcx = fx + col_w * (i - 1) + col_w * 0.5
        ctx:text(fcx, ty + 28, tostring(hf.hour or ""), 15, subtle, "center")
        draw_weather_icon(ctx, fcx, ty + 82, 34, hf.condition, hf.is_day)
        ctx:text(fcx, ty + 124, string.format("%.0f°", tonumber(hf.temp) or 0), 20, white, "center")
      end
    end
  else
    -- Classic 1-slot v1 readout
    ctx:text(rx, ty + 12, temp_1, 40, white, "left")
    ctx:text(rx, ty + 58, hl_s, 15, subtle, "left")
    draw_weather_icon(ctx, tx + tw - 42, ty + 38, 52, weather.condition, weather.is_day)

    if weather.ready and type(weather.hourly) == "table" and #weather.hourly > 0 then
      local col_w = rw / 3.0
      for i = 1, math.min(3, #weather.hourly) do
        local hf = weather.hourly[i]
        local fcx = rx + col_w * (i - 1) + col_w * 0.5
        ctx:text(fcx, ty + 90, tostring(hf.hour or ""), 13, subtle, "center")
        draw_weather_icon(ctx, fcx, ty + 118, 22, hf.condition, hf.is_day)
        ctx:text(fcx, ty + 134, string.format("%.0f°", tonumber(hf.temp) or 0), 15, white, "center")
      end
    end
  end

  ctx:clear_clip()
end
