local metrics = { cpu_percent = 0, memory_used_gb = 0, memory_total_gb = 16, gpu_percent = 0 }
local connected = false

function on_event(topic, payload)
  if topic == "host/system/metrics" and type(payload) == "table" then
    metrics = payload
    connected = true
  elseif topic == "host/connection" and type(payload) == "table" then
    connected = payload.connected == true
  end
end

local function draw_bar(ctx, x, y, w, h, frac, color)
  ctx:round_rect(x, y, w, h, 5, { 0.14, 0.18, 0.26, 0.85 }, 0, { 0, 0, 0, 0 })
  local fw = math.max(8, w * math.max(0.0, math.min(1.0, frac)))
  ctx:round_rect(x, y, fw, h, 5, color, 0, { 0, 0, 0, 0 })
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end
  local accent = module.color("accent", "#58f29c")

  ctx:clip(sr.x, sr.y, sr.w, sr.h)
  ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.72 }, 1.5, { accent[1], accent[2], accent[3], 0.42 })
  ctx:text(sr.x + 20, sr.y + 14, "SYSTEM", 18, { 0.68, 0.82, 0.94, 0.9 }, "left")
  ctx:text(sr.x + sr.w - 20, sr.y + 14, connected and "LIVE" or "STANDALONE", 16, connected and accent or { 0.7, 0.7, 0.75, 0.7 }, "right")

  local cpu = tonumber(metrics.cpu_percent) or 0
  local mem_used = tonumber(metrics.memory_used_gb) or 0
  local mem_tot = math.max(1, tonumber(metrics.memory_total_gb) or 16)
  local gpu = tonumber(metrics.gpu_percent) or 0
  local bar_w = sr.w - 135

  ctx:text(sr.x + 20, sr.y + 48, string.format("CPU %2.0f%%", cpu), 20, { 0.95, 0.98, 1.0, 0.95 }, "left")
  draw_bar(ctx, sr.x + 115, sr.y + 53, bar_w, 12, cpu / 100.0, accent)

  ctx:text(sr.x + 20, sr.y + 82, string.format("RAM %2.0fG", mem_used), 20, { 0.95, 0.98, 1.0, 0.95 }, "left")
  draw_bar(ctx, sr.x + 115, sr.y + 87, bar_w, 12, mem_used / mem_tot, { 0.32, 0.78, 1.0, 0.95 })

  ctx:text(sr.x + 20, sr.y + 116, string.format("GPU %2.0f%%", gpu), 20, { 0.95, 0.98, 1.0, 0.95 }, "left")
  draw_bar(ctx, sr.x + 115, sr.y + 121, bar_w, 12, gpu / 100.0, { 0.78, 0.52, 1.0, 0.95 })
  ctx:clear_clip()
end
