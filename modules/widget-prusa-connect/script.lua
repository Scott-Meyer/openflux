-- Prusa Connect Widget + Hold-a-Key Live Camera Glance!
-- Shows nozzle/bed temps, print progress & ETA in its top-bar tile, and displays a live
-- camera feed overlay across the keyboard ONLY while you hold the configured glance key.

local status = {
  name = "Prusa Printer",
  state = "UNCONFIGURED",
  progress = 0,
  time_remaining_min = 0,
  nozzle_temp = 0,
  nozzle_target = 0,
  bed_temp = 0,
  bed_target = 0,
  job_name = "Set API token in Settings",
  camera_path = "",
  camera_kb_path = "/home/root/openflux/media/prusa_cam_live.jpg",
}

local peeking = false

function on_event(topic, payload)
  if topic == "modules/prusa-connect/status" and type(payload) == "table" then
    for k, v in pairs(payload) do
      status[k] = v
    end
  elseif topic == "input/keys/event" and type(payload) == "table" then
    local target_key = math.floor(tonumber(module.config().peek_key_index) or 13)
    if payload.key_index == target_key then
      local next_peek = (payload.kind == "press")
      if next_peek ~= peeking then
        peeking = next_peek
        bus.publish("modules/prusa-connect/peek", { active = peeking }, true)
      end
    end
  end
end

local function fmt_eta(mins)
  local m = math.floor(tonumber(mins) or 0)
  if m <= 0 then return "--" end
  local h = math.floor(m / 60)
  local rem = m % 60
  if h > 0 then
    return string.format("%dh %02dm", h, rem)
  end
  return string.format("%dm", rem)
end

function on_render(ctx, stage)
  local orange = { 0.98, 0.41, 0.19, 0.98 }
  local state_str = tostring(status.state or "IDLE")
  local is_printing = (state_str == "PRINTING" or state_str == "BUSY")
  local badge_col = is_printing and { 0.34, 0.95, 0.58, 1.0 }
    or (state_str == "ERROR" and { 1.0, 0.35, 0.38, 1.0 } or orange)

  if stage == "tile" then
    local sr = module.slot_rect()
    if not sr then return end
    ctx:clip(sr.x, sr.y, sr.w, sr.h)
    ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.76 }, 1.6, { orange[1], orange[2], orange[3], 0.52 })

    local title = tostring(status.name or "Prusa")
    if #title > 16 then title = string.sub(title, 1, 15) .. "..." end
    ctx:text(sr.x + 18, sr.y + 14, title, 20, orange, "left")
    ctx:text(sr.x + sr.w - 18, sr.y + 15, state_str, 16, badge_col, "right")

    local noz = math.floor((tonumber(status.nozzle_temp) or 0) + 0.5)
    local noz_t = math.floor((tonumber(status.nozzle_target) or 0) + 0.5)
    local bed = math.floor((tonumber(status.bed_temp) or 0) + 0.5)
    local bed_t = math.floor((tonumber(status.bed_target) or 0) + 0.5)
    ctx:text(sr.x + 18, sr.y + 46, string.format("NOZ %d/%dC", noz, noz_t), 20, { 0.96, 0.98, 1.0, 0.95 }, "left")
    ctx:text(sr.x + sr.w - 18, sr.y + 46, string.format("BED %d/%dC", bed, bed_t), 20, { 0.80, 0.88, 0.98, 0.92 }, "right")

    local job = tostring(status.job_name or "")
    if #job > 22 then job = string.sub(job, 1, 21) .. "..." end
    ctx:text(sr.x + 18, sr.y + 78, job, 17, { 0.75, 0.82, 0.90, 0.86 }, "left")
    if is_printing then
      ctx:text(sr.x + sr.w - 18, sr.y + 78, fmt_eta(status.time_remaining_min), 17, { 0.95, 0.95, 1.0, 0.92 }, "right")
    end

    local prog = math.max(0, math.min(100, tonumber(status.progress) or 0))
    local bar_w = sr.w - 92
    ctx:round_rect(sr.x + 18, sr.y + 114, bar_w, 12, 6, { 0.14, 0.17, 0.24, 0.9 }, 0, { 0, 0, 0, 0 })
    if prog > 0 then
      ctx:round_rect(sr.x + 18, sr.y + 114, math.max(10, bar_w * (prog / 100.0)), 12, 6, orange, 0, { 0, 0, 0, 0 })
    end
    ctx:text(sr.x + sr.w - 18, sr.y + 109, string.format("%d%%", math.floor(prog + 0.5)), 20, { 1, 1, 1, 0.95 }, "right")
    ctx:clear_clip()
  elseif stage == "world_overlay" and peeking then
    -- Live Camera Glance overlay shown ONLY while holding the configured button!
    local ox, oy, ow, oh = 360, 225, 1200, 740
    ctx:round_rect(ox, oy, ow, oh, 24, { 0.03, 0.05, 0.09, 0.94 }, 2.5, orange)

    -- Render latest camera snapshot JPEG (keyboard path on Linux, local host path on Mac preview)
    local cam_path = tostring(status.camera_kb_path or "")
    if type(status.camera_path) == "string" and #status.camera_path > 0 and string.sub(module.dir(), 1, 5) ~= "/usr/" and string.sub(module.dir(), 1, 6) ~= "/home/" then
      cam_path = status.camera_path
    end
    local img_x, img_y, img_w, img_h = ox + 20, oy + 62, ow - 40, oh - 134
    ctx:round_rect(img_x, img_y, img_w, img_h, 14, { 0.07, 0.09, 0.14, 1.0 }, 1.0, { 1, 1, 1, 0.12 })
    if #cam_path > 0 then
      ctx:image(img_x, img_y, img_w, img_h, cam_path, { 1, 1, 1, 1 })
    else
      ctx:text(ox + ow * 0.5, oy + oh * 0.48, "Waiting for Prusa camera snapshot...", 28, { 0.78, 0.85, 0.95, 0.8 }, "center")
    end

    -- Header + bottom live telemetry bar
    ctx:text(ox + 28, oy + 18, "LIVE CAMERA  •  " .. tostring(status.name or "Prusa"), 24, orange, "left")
    ctx:text(ox + ow - 28, oy + 20, "Release key to close", 18, { 0.75, 0.82, 0.92, 0.8 }, "right")

    local footer_y = oy + oh - 54
    local noz = math.floor((tonumber(status.nozzle_temp) or 0) + 0.5)
    local noz_t = math.floor((tonumber(status.nozzle_target) or 0) + 0.5)
    local bed = math.floor((tonumber(status.bed_temp) or 0) + 0.5)
    local bed_t = math.floor((tonumber(status.bed_target) or 0) + 0.5)
    local prog = math.floor((tonumber(status.progress) or 0) + 0.5)
    ctx:text(
      ox + 28,
      footer_y,
      string.format("%s   |   Nozzle %d/%dC   |   Bed %d/%dC   |   %d%% (%s)", tostring(status.state or "IDLE"), noz, noz_t, bed, bed_t, prog, fmt_eta(status.time_remaining_min)),
      22,
      { 0.96, 0.98, 1.0, 0.96 },
      "left"
    )
  end
end
