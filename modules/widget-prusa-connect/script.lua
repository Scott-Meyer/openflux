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
local stream_live = false
local anim_t = 0.0
local prev_printer_state = ""

function on_update(dt)
  anim_t = anim_t + dt
end

function on_event(topic, payload)
  if topic == "modules/prusa-connect/status" and type(payload) == "table" then
    local next_state = tostring(payload.state or "")
    if prev_printer_state == "PRINTING" and (next_state == "FINISHED" or next_state == "IDLE") then
      bus.publish("ui/toast", {
        id = "prusa-done",
        icon = "check",
        title = tostring(payload.name or "Prusa") .. " Complete!",
        message = tostring(payload.job_name or "Print finished"),
        color = "#ff6a30",
        duration_ms = 5000,
      }, false)
    end
    if #next_state > 0 then prev_printer_state = next_state end
    for k, v in pairs(payload) do
      status[k] = v
    end
  elseif topic == "modules/streams/prusa-cam" and type(payload) == "table" then
    stream_live = (payload.active == true)
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

    local noz = math.floor((tonumber(status.nozzle_temp) or 0) + 0.5)
    local noz_t = math.floor((tonumber(status.nozzle_target) or 0) + 0.5)
    local bed = math.floor((tonumber(status.bed_temp) or 0) + 0.5)
    local bed_t = math.floor((tonumber(status.bed_target) or 0) + 0.5)
    local prog = math.max(0, math.min(100, tonumber(status.progress) or 0))

    if sr.w < 260 then
      -- Half-slot compact Prusa card
      ctx:text(sr.x + 12, sr.y + 14, "PRUSA", 15, orange, "left")
      ctx:text(sr.x + sr.w - 12, sr.y + 14, string.format("%d%%", math.floor(prog + 0.5)), 18, badge_col, "right")
      ctx:text(sr.x + 12, sr.y + 46, string.format("NOZ %d°", noz), 18, { 0.96, 0.98, 1.0, 0.95 }, "left")
      ctx:text(sr.x + 12, sr.y + 76, string.format("BED %d°", bed), 18, { 0.80, 0.88, 0.98, 0.92 }, "left")
      local bw = sr.w - 24
      ctx:round_rect(sr.x + 12, sr.y + 114, bw, 10, 5, { 0.14, 0.17, 0.24, 0.9 }, 0, { 0, 0, 0, 0 })
      if prog > 0 then
        ctx:round_rect(sr.x + 12, sr.y + 114, math.max(8, bw * (prog / 100.0)), 10, 5, orange, 0, { 0, 0, 0, 0 })
      end
      ctx:clear_clip()
      return
    end

    local max_title = sr.w > 580 and 36 or 16
    local title = tostring(status.name or "Prusa")
    if #title > max_title then title = string.sub(title, 1, max_title - 1) .. "..." end
    ctx:text(sr.x + 18, sr.y + 14, title, 20, orange, "left")
    ctx:text(sr.x + sr.w - 18, sr.y + 15, state_str, 16, badge_col, "right")

    ctx:text(sr.x + 18, sr.y + 46, string.format("NOZ %d/%dC", noz, noz_t), 20, { 0.96, 0.98, 1.0, 0.95 }, "left")
    ctx:text(sr.x + sr.w - 18, sr.y + 46, string.format("BED %d/%dC", bed, bed_t), 20, { 0.80, 0.88, 0.98, 0.92 }, "right")

    local max_job = sr.w > 580 and 54 or 22
    local job = tostring(status.job_name or "")
    if #job > max_job then job = string.sub(job, 1, max_job - 1) .. "..." end
    ctx:text(sr.x + 18, sr.y + 78, job, 17, { 0.75, 0.82, 0.90, 0.86 }, "left")
    if is_printing then
      ctx:text(sr.x + sr.w - 18, sr.y + 78, fmt_eta(status.time_remaining_min), 17, { 0.95, 0.95, 1.0, 0.92 }, "right")
    end

    local bar_w = sr.w - 92
    ctx:round_rect(sr.x + 18, sr.y + 114, bar_w, 12, 6, { 0.14, 0.17, 0.24, 0.9 }, 0, { 0, 0, 0, 0 })
    if prog > 0 then
      ctx:round_rect(sr.x + 18, sr.y + 114, math.max(10, bar_w * (prog / 100.0)), 12, 6, orange, 0, { 0, 0, 0, 0 })
    end
    ctx:text(sr.x + sr.w - 18, sr.y + 109, string.format("%d%%", math.floor(prog + 0.5)), 20, { 1, 1, 1, 0.95 }, "right")
    ctx:clear_clip()
  elseif stage == "world_overlay" and peeking then
    -- Sleek centered Camera Glance popup card shown ONLY while holding PRT!
    -- Ocean waves & top bar remain visible behind it; shows cached snapshot immediately for
    -- instant quick clicks and upgrades smoothly in-place to 20 FPS hardware H.264 video!
    local ox, oy, ow, oh = 360, 225, 1200, 740
    ctx:round_rect(ox, oy, ow, oh, 24, { 0.03, 0.05, 0.09, 0.94 }, 2.5, orange)

    local img_x, img_y, img_w, img_h = ox + 20, oy + 62, ow - 40, oh - 134
    ctx:round_rect(img_x, img_y, img_w, img_h, 14, { 0.07, 0.09, 0.14, 1.0 }, 1.0, { 1, 1, 1, 0.12 })

    local cam_path = tostring(status.camera_kb_path or "")
    if type(status.camera_path) == "string" and #status.camera_path > 0 and string.sub(module.dir(), 1, 5) ~= "/usr/" and string.sub(module.dir(), 1, 6) ~= "/home/" then
      cam_path = status.camera_path
    end
    local cmode = tostring(status.camera_mode or "")
    if stream_live then
      -- Live 20 FPS hardware-decoded H.264 right inside the popup rectangle!
      ctx:stream_rect(img_x, img_y, img_w, img_h, "prusa-cam", 14)
    elseif #cam_path > 0 and cmode ~= "none" then
      -- Instant cached snapshot on quick click or when RTSP is offline
      ctx:image(img_x, img_y, img_w, img_h, cam_path, { 1, 1, 1, 1 })
    elseif cmode == "live" then
      ctx:text(ox + ow * 0.5, oy + oh * 0.46, "Waiting for Buddy3D RTSP camera…", 26, { 0.92, 0.96, 1.0, 0.92 }, "center")
      ctx:text(ox + ow * 0.5, oy + oh * 0.54, "Check RTSP on your camera (192.168.1.77:554)", 18, { 0.68, 0.78, 0.90, 0.78 }, "center")
    else
      ctx:text(ox + ow * 0.5, oy + oh * 0.44, "No live camera URL configured yet", 28, { 0.88, 0.92, 0.98, 0.9 }, "center")
      ctx:text(ox + ow * 0.5, oy + oh * 0.52, "Enable RTSP on your Buddy3D Camera and set Camera URL in Studio", 20, { 0.68, 0.78, 0.90, 0.78 }, "center")
    end

    -- Header with pulsing red LIVE dot when streaming
    if stream_live then
      local pulse = (math.floor(anim_t * 3.0) % 2 == 0) and 1.0 or 0.45
      ctx:circle(ox + 34, oy + 29, 7, { 1.0, 0.24, 0.26, pulse })
      ctx:text(ox + 50, oy + 18, "LIVE CAMERA  •  " .. tostring(status.name or "Prusa"), 22, orange, "left")
    else
      ctx:text(ox + 28, oy + 18, "PRINT CAMERA  •  " .. tostring(status.name or "Prusa"), 22, orange, "left")
    end
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
