local total_sec = 25 * 60
local remaining_sec = 25 * 60
local running = true

function on_init()
  local mins = tonumber(module.config().work_minutes) or 25
  total_sec = mins * 60
  remaining_sec = total_sec
end

function on_event(topic, payload)
  if topic == "modules/widget-pomodoro/command" and type(payload) == "table" then
    if payload.action == "toggle" then running = not running
    elseif payload.action == "reset" then remaining_sec = total_sec
    elseif payload.action == "finish" then remaining_sec = 0.05 end
  end
end

function on_update(dt)
  if not running then return end
  if remaining_sec > 0 then
    remaining_sec = math.max(0, remaining_sec - dt)
    if remaining_sec == 0 then
      running = false
      bus.publish("modules/widget-pomodoro/state", { status = "completed", remaining_sec = 0 }, true)
      bus.publish("fx/surface/impulse", { x = 960, y = 540, radius = 140, force = 1.6, kind = "shockwave", source = "module:widget-pomodoro" }, false)
    end
  end
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end
  local accent = { 1.0, 0.42, 0.42, 0.95 }
  local mins = math.floor(remaining_sec / 60)
  local secs = math.floor(remaining_sec % 60)
  local progress = 1.0 - (remaining_sec / math.max(1.0, total_sec))

  ctx:clip(sr.x, sr.y, sr.w, sr.h)
  ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.72 }, 1.5, { accent[1], accent[2], accent[3], 0.48 })
  ctx:text(sr.x + 22, sr.y + 18, "POMODORO", 18, accent, "left")
  ctx:text(sr.x + 22, sr.y + 52, string.format("%02d:%02d", mins, secs), 50, { 1.0, 1.0, 1.0, 0.98 }, "left")
  local rx = sr.x + sr.w - 60
  local ry = sr.y + sr.h * 0.52
  ctx:circle(rx, ry, 34, { 0.12, 0.14, 0.2, 0.6 }, 5.0, { 0.3, 0.35, 0.45, 0.35 })
  ctx:arc(rx, ry, 34, -90.0, -90.0 + progress * 360.0, 6.0, accent)
  ctx:clear_clip()
end
