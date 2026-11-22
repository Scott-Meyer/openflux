-- Responsive Now Playing Widget (`widget-media`)
-- Supports 1x (classic player), 2x, and 3x (widescreen untruncated track + wide visualizer + scrubber).

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

local function fmt_mmss(sec)
  local s = math.max(0, math.floor(tonumber(sec) or 0))
  return string.format("%d:%02d", math.floor(s / 60), s % 60)
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end
  local accent = module.color("accent", "#ff64b4")
  local wide = sr.w > 580
  local max_t = math.max(18, math.floor((sr.w - 90) / 16))
  local max_a = math.max(22, math.floor((sr.w - 90) / 12))

  ctx:clip(sr.x, sr.y, sr.w, sr.h)
  ctx:text(sr.x + 18, sr.y + 16, media.playing and "NOW PLAYING" or "PAUSED", 15, accent, "left")

  -- Animated equalizer bars on the right (wider spectrum on 2x/3x spans!)
  local eq_count = wide and 8 or 4
  for i = 0, eq_count - 1 do
    local bh = media.playing and (12 + 26 * (0.5 + 0.5 * math.sin(anim * 7.0 + i * 1.1))) or 8
    ctx:round_rect(sr.x + sr.w - (eq_count * 10 + 18) + i * 10, sr.y + 44 - bh, 6, bh, 3, accent, 0, { 0, 0, 0, 0 })
  end

  local title = tostring(media.title or "Nothing Playing")
  if #title > max_t then title = string.sub(title, 1, max_t - 1) .. "..." end
  local artist = tostring(media.artist or "")
  if #artist > max_a then artist = string.sub(artist, 1, max_a - 1) .. "..." end

  ctx:text(sr.x + 20, sr.y + 44, title, wide and 34 or 28, { 1.0, 1.0, 1.0, 0.98 }, "left")
  ctx:text(sr.x + 20, sr.y + (wide and 86 or 84), artist, wide and 22 or 20, { 0.76, 0.82, 0.92, 0.86 }, "left")

  local len = math.max(1.0, tonumber(media.length_sec) or 1.0)
  local pos = math.max(0.0, math.min(len, tonumber(media.position_sec) or 0.0))
  local bar_x = sr.x + (wide and 74 or 20)
  local bar_w = sr.w - (wide and 148 or 40)
  if wide then
    ctx:text(sr.x + 20, sr.y + 122, fmt_mmss(pos), 14, { 0.75, 0.82, 0.9, 0.8 }, "left")
    ctx:text(sr.x + sr.w - 20, sr.y + 122, fmt_mmss(len), 14, { 0.75, 0.82, 0.9, 0.8 }, "right")
  end
  ctx:round_rect(bar_x, sr.y + 126, bar_w, 8, 4, { 0.16, 0.19, 0.26, 0.85 }, 0, { 0, 0, 0, 0 })
  ctx:round_rect(bar_x, sr.y + 126, math.max(6, bar_w * (pos / len)), 8, 4, accent, 0, { 0, 0, 0, 0 })
  ctx:clear_clip()
end
