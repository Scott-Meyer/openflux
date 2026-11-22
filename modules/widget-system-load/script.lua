-- Exact v1 System Load Tile (`TileType::HwStatus`):
-- Clean 3x2 hairline grid for CPU, GPU, Upload / Memory, VRAM, Download.

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
  local gx0 = tx + 8
  local gy0 = ty + 8
  local cw = math.floor((tw - 16) / 3)
  local ch = math.floor((th - 16) / 2)
  local grid_w = cw * 3
  local grid_h = ch * 2

  local glass_bg = { 0.03, 0.04, 0.06, 0.56 }
  local hairline = { 1.0, 1.0, 1.0, 0.16 }
  local white = { 1.0, 1.0, 1.0, 0.98 }
  local subtle = { 0.72, 0.78, 0.86, 0.78 }

  ctx:clip(tx, ty, tw, th)

  -- 3x2 hairline outer box and dividers (matching v1 HwStatus pixel-for-pixel, frameless over background)
  ctx:round_rect(gx0, gy0, grid_w, grid_h, 4, { 0, 0, 0, 0 }, 1.2, hairline)
  ctx:rect(gx0 + cw, gy0, 1, grid_h, hairline)
  ctx:rect(gx0 + cw * 2, gy0, 1, grid_h, hairline)
  ctx:rect(gx0, gy0 + ch, grid_w, 1, hairline)

  -- Row 0: CPU | GPU | Upload
  local cpu_s = string.format("%.0f%%", tonumber(metrics.cpu_percent) or 0)
  ctx:text(gx0 + 12, gy0 + 10, "CPU", 14, subtle, "left")
  ctx:text(gx0 + 12, gy0 + 34, cpu_s, 30, white, "left")

  local gpu_s = string.format("%.0f%%", tonumber(metrics.gpu_percent) or 0)
  ctx:text(gx0 + cw + 12, gy0 + 10, "GPU", 14, subtle, "left")
  ctx:text(gx0 + cw + 12, gy0 + 34, gpu_s, 30, white, "left")

  local up_s = tostring(metrics.net_up or "0 KB/s")
  if up_s == "" then up_s = "0 KB/s" end
  ctx:text(gx0 + cw * 2 + 12, gy0 + 10, "Upload", 14, subtle, "left")
  ctx:text(gx0 + cw * 2 + 12, gy0 + 36, up_s, 22, white, "left")

  -- Row 1: Memory | VRAM | Download
  local mem_used = tonumber(metrics.memory_used_gb) or 0
  local mem_tot = tonumber(metrics.memory_total_gb) or 0
  local mem_pct = (mem_tot > 0) and math.min(100, (mem_used / mem_tot) * 100.0) or 0
  local mem_pct_s = string.format("%.0f%%", mem_pct)
  local mem_sub = (mem_tot > 0) and string.format("%.0f/%.0fG", mem_used, mem_tot) or "--"
  ctx:text(gx0 + 12, gy0 + ch + 10, "Memory", 14, subtle, "left")
  ctx:text(gx0 + 12, gy0 + ch + 32, mem_pct_s, 28, white, "left")
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

  local down_s = tostring(metrics.net_down or "0 KB/s")
  if down_s == "" then down_s = "0 KB/s" end
  ctx:text(gx0 + cw * 2 + 12, gy0 + ch + 10, "Download", 14, subtle, "left")
  ctx:text(gx0 + cw * 2 + 12, gy0 + ch + 36, down_s, 22, white, "left")

  ctx:clear_clip()
end
