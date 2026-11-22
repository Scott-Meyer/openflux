local media = { title = "Nothing Playing", artist = "Connect host media", playing = false, length_sec = 1, position_sec = 0 }
local anim = 0

function on_event(topic, payload)
  if topic == "host/media/now_playing" and type(payload) == "table" then
    media = payload
  end
end

function on_update(dt)
  anim = anim + dt
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end
  local accent = module.color("accent", "#ff64b4")

  ctx:clip(sr.x, sr.y, sr.w, sr.h)
  ctx:text(sr.x + 16, sr.y + 16, media.playing and "NOW PLAYING" or "PAUSED", 15, accent, "left")

  -- Animated equalizer bars on the right
  for i = 0, 3 do
    local bh = media.playing and (12 + 22 * (0.5 + 0.5 * math.sin(anim * 7.0 + i * 1.4))) or 8
    ctx:round_rect(sr.x + sr.w - 58 + i * 10, sr.y + 42 - bh, 6, bh, 3, accent, 0, { 0, 0, 0, 0 })
  end

  local title = tostring(media.title or "Nothing Playing")
  if #title > 20 then title = string.sub(title, 1, 19) .. "..." end
  local artist = tostring(media.artist or "")
  if #artist > 24 then artist = string.sub(artist, 1, 23) .. "..." end

  ctx:text(sr.x + 20, sr.y + 46, title, 28, { 1.0, 1.0, 1.0, 0.98 }, "left")
  ctx:text(sr.x + 20, sr.y + 84, artist, 20, { 0.76, 0.82, 0.92, 0.86 }, "left")

  local len = math.max(1.0, tonumber(media.length_sec) or 1.0)
  local pos = math.max(0.0, math.min(len, tonumber(media.position_sec) or 0.0))
  local bar_w = sr.w - 40
  ctx:round_rect(sr.x + 20, sr.y + 124, bar_w, 8, 4, { 0.16, 0.19, 0.26, 0.85 }, 0, { 0, 0, 0, 0 })
  ctx:round_rect(sr.x + 20, sr.y + 124, math.max(6, bar_w * (pos / len)), 8, 4, accent, 0, { 0, 0, 0, 0 })
  ctx:clear_clip()
end
