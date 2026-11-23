-- Responsive System Load Widget (`widget-system-load`)
-- Supports 0.5x (compact CPU/GPU/RAM stack), 1x (classic v1 3x2 hairline grid), and 2x (widescreen with live bars).

local metrics = {
  cpu_percent = 0,
  memory_used_gb = 0,
  memory_total_gb = 0,
  gpu_percent = 0,
  gpu_unified = false,
  vram_used_gb = nil,
  vram_total_gb = nil,
  net_down = "0 KB/s",
  net_up = "0 KB/s",
}

function on_init()
  local cur = bus.get("host/system/metrics")
  if type(cur) == "table" then metrics = cur end
end

function on_event(topic, payload)
  if topic == "host/system/metrics" and type(payload) == "table" then
    metrics = payload
  end
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local sr = module.slot_rect()
  if not sr then return end

  local tx, ty, tw, th = sr.x, sr.y, sr.w, sr.h
  local accent = module.color("accent", "#58f29c")
  local hairline = { 1.0, 1.0, 1.0, 0.16 }
  local white = { 1.0, 1.0, 1.0, 0.98 }
  local subtle = { 0.72, 0.78, 0.86, 0.78 }

  local cpu_pct = tonumber(metrics.cpu_percent) or 0
  local gpu_pct = tonumber(metrics.gpu_percent) or 0
  local mem_used = tonumber(metrics.memory_used_gb) or 0
  local mem_tot = tonumber(metrics.memory_total_gb) or 0
  local mem_pct = (mem_tot > 0) and math.min(100, (mem_used / mem_tot) * 100.0) or 0
  local up_s = tostring(metrics.net_up or "0 KB/s")
  if up_s == "" then up_s = "0 KB/s" end
  local down_s = tostring(metrics.net_down or "0 KB/s")
  if down_s == "" then down_s = "0 KB/s" end

  ctx:clip(tx, ty, tw, th)

  if tw < 260 then
    -- Half-slot compact 3-row telemetry stack
    local rows = {
      { "CPU", string.format("%.0f%%", cpu_pct), cpu_pct },
      { "GPU", string.format("%.0f%%", gpu_pct), gpu_pct },
      { "RAM", string.format("%.0f%%", mem_pct), mem_pct },
    }
    for i, r in ipairs(rows) do
      local ry = ty + 14 + (i - 1) * 48
      ctx:text(tx + 10, ry, r[1], 14, subtle, "left")
      ctx:text(tx + tw - 10, ry - 2, r[2], 20, white, "right")
      local bw = tw - 20
      ctx:round_rect(tx + 10, ry + 26, bw, 6, 3, { 1, 1, 1, 0.12 }, 0, { 0, 0, 0, 0 })
      ctx:round_rect(tx + 10, ry + 26, math.max(4, bw * math.min(1, r[3] / 100.0)), 6, 3, accent, 0, { 0, 0, 0, 0 })
    end
  elseif tw > 580 then
    -- Widescreen 2-slot dashboard with live fill bars + network throughput
    local left_w = tw * 0.62
    local bar_w = left_w - 130
    local bars = {
      { "CPU", string.format("%.0f%%", cpu_pct), cpu_pct },
      { "GPU", string.format("%.0f%%", gpu_pct), gpu_pct },
      { "MEM", (mem_tot > 0) and string.format("%.0f/%.0fG", mem_used, mem_tot) or "--", mem_pct },
    }
    for i, b in ipairs(bars) do
      local ry = ty + 22 + (i - 1) * 48
      ctx:text(tx + 16, ry, b[1], 16, subtle, "left")
      ctx:round_rect(tx + 72, ry + 5, bar_w, 12, 6, { 1, 1, 1, 0.12 }, 0, { 0, 0, 0, 0 })
      ctx:round_rect(tx + 72, ry + 5, math.max(8, bar_w * math.min(1, b[3] / 100.0)), 12, 6, accent, 0, { 0, 0, 0, 0 })
      ctx:text(tx + left_w - 6, ry - 2, b[2], 18, white, "right")
    end
    local div_x = tx + left_w + 16
    ctx:rect(div_x, ty + 18, 1, th - 36, hairline)
    local nx = div_x + 22
    ctx:text(nx, ty + 20, "UPLOAD", 14, subtle, "left")
    ctx:text(nx, ty + 42, up_s, 26, white, "left")
    ctx:text(nx, ty + 94, "DOWNLOAD", 14, subtle, "left")
    ctx:text(nx, ty + 116, down_s, 26, white, "left")
  else
    -- Classic 1-slot v1 3x2 hairline grid
    local gx0 = tx + 8
    local gy0 = ty + 8
    local cw = math.floor((tw - 16) / 3)
    local ch = math.floor((th - 16) / 2)
    local grid_w = cw * 3
    local grid_h = ch * 2

    ctx:round_rect(gx0, gy0, grid_w, grid_h, 4, { 0, 0, 0, 0 }, 1.2, hairline)
    ctx:rect(gx0 + cw, gy0, 1, grid_h, hairline)
    ctx:rect(gx0 + cw * 2, gy0, 1, grid_h, hairline)
    ctx:rect(gx0, gy0 + ch, grid_w, 1, hairline)

    ctx:text(gx0 + 12, gy0 + 10, "CPU", 14, subtle, "left")
    ctx:text(gx0 + 12, gy0 + 34, string.format("%.0f%%", cpu_pct), 30, white, "left")

    ctx:text(gx0 + cw + 12, gy0 + 10, "GPU", 14, subtle, "left")
    ctx:text(gx0 + cw + 12, gy0 + 34, string.format("%.0f%%", gpu_pct), 30, white, "left")

    ctx:text(gx0 + cw * 2 + 12, gy0 + 10, "Upload", 14, subtle, "left")
    ctx:text(gx0 + cw * 2 + 12, gy0 + 36, up_s, 22, white, "left")

    local mem_sub = (mem_tot > 0) and string.format("%.0f/%.0fG", mem_used, mem_tot) or "--"
    ctx:text(gx0 + 12, gy0 + ch + 10, "Memory", 14, subtle, "left")
    ctx:text(gx0 + 12, gy0 + ch + 32, string.format("%.0f%%", mem_pct), 28, white, "left")
    ctx:text(gx0 + 12, gy0 + ch + 66, mem_sub, 13, subtle, "left")

    ctx:text(gx0 + cw + 12, gy0 + ch + 10, "VRAM", 14, subtle, "left")
    local vu = tonumber(metrics.vram_used_gb)
    local vt = tonumber(metrics.vram_total_gb)
    if vu and vt and vt > 0 then
      local vpct = math.min(100, (vu / vt) * 100.0)
      ctx:text(gx0 + cw + 12, gy0 + ch + 32, string.format("%.0f%%", vpct), 28, white, "left")
      ctx:text(gx0 + cw + 12, gy0 + ch + 66, string.format("%.1f/%.1fG", vu, vt), 13, subtle, "left")
    elseif vu and metrics.gpu_unified then
      ctx:text(gx0 + cw + 12, gy0 + ch + 34, string.format("%.1fG", vu), 24, white, "left")
      ctx:text(gx0 + cw + 12, gy0 + ch + 66, "Unified", 13, subtle, "left")
    else
      ctx:text(gx0 + cw + 12, gy0 + ch + 34, "--", 24, { 0.58, 0.64, 0.72, 0.9 }, "left")
      ctx:text(gx0 + cw + 12, gy0 + ch + 66, metrics.gpu_unified and "Unified" or "Unavailable", 13, subtle, "left")
    end

    ctx:text(gx0 + cw * 2 + 12, gy0 + ch + 10, "Download", 14, subtle, "left")
    ctx:text(gx0 + cw * 2 + 12, gy0 + ch + 36, down_s, 22, white, "left")
  end

  ctx:clear_clip()
end
