-- Responsive Digital Clock Widget (`widget-clock`)
-- Follows the host computer's exact time & timezone via retained `host/time` (so even an
-- embedded keyboard without an RTC or local timezone always shows the exact local time & date!).
-- Supports 0.5x (compact time + day), 1x (classic v1 time + full date), and 2x (widescreen + seconds).

local synced_unix = nil
local synced_tz_offset = 0
local synced_os_time = 0

local function apply_host_time(p)
  if type(p) ~= "table" then return end
  local u = tonumber(p.unix_sec)
  if u and u > 1000000000 then
    synced_unix = math.floor(u)
    synced_tz_offset = math.floor(tonumber(p.tz_offset_sec) or 0)
    synced_os_time = os.time()
  end
end

function on_init()
  apply_host_time(bus.get("host/time"))
end

function on_event(topic, payload)
  if topic == "host/time" then
    apply_host_time(payload)
  end
end

local function current_local_time()
  if synced_unix then
    local elapsed = math.max(0, os.time() - synced_os_time)
    local local_ts = synced_unix + synced_tz_offset + elapsed
    return os.date("!*t", local_ts), local_ts, true
  end
  return os.date("*t"), os.time(), false
end

local function fmt_date(fmt, ts, is_utc_shifted)
  if is_utc_shifted then
    return os.date("!" .. fmt, ts)
  end
  return os.date(fmt, ts)
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end

  local tx, ty, tw, th = sr.x, sr.y, sr.w, sr.h
  local use_24h = module.config().use_24h == true
  local now, ts, shifted = current_local_time()
  local white = { 1.0, 1.0, 1.0, 0.98 }
  local subtle = { 0.74, 0.80, 0.88, 0.78 }
  local cx = tx + tw * 0.5

  ctx:clip(tx, ty, tw, th)

  if tw < 260 then
    -- Half-slot compact layout
    local h_val = use_24h and now.hour or ((now.hour % 12 == 0) and 12 or (now.hour % 12))
    local time_s = use_24h and string.format("%02d:%02d", h_val, now.min) or string.format("%d:%02d", h_val, now.min)
    ctx:text(cx, ty + 30, time_s, 48, white, "center")
    local day_s = string.upper(fmt_date("%a %d", ts, shifted))
    local sub = use_24h and day_s or (day_s .. "  " .. (now.hour >= 12 and "PM" or "AM"))
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
    ctx:text(rx, ty + 42, string.upper(fmt_date("%A", ts, shifted)), 26, white, "left")
    ctx:text(rx, ty + 82, string.upper(fmt_date("%B %d, %Y", ts, shifted)), 18, subtle, "left")
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
    local date_s = string.upper(fmt_date("%A, %b %d, %Y", ts, shifted))
    ctx:text(cx, ty + th - 44, date_s, 15, subtle, "center")
  end

  ctx:clear_clip()
end
