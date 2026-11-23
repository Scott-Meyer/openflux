-- Prusa Connect / PrusaLink Live Widget (`widget-prusa-connect`)
-- Supports 0.5x..3x responsive top-bar tile layouts, configurable Hold-to-Peek camera key,
-- optional automatic live video while printing (`off` | `popup` | `background`),
-- centered hardware H.264 popup via `ctx:stream_rect(...)`, and `ui/toast` print-completion alerts!

local status = {
  state = "OFFLINE",
  name = "Prusa",
  progress = 0,
  nozzle_temp = 0,
  nozzle_target = 0,
  bed_temp = 0,
  bed_target = 0,
  time_remaining_min = 0,
  job_name = "",
  camera_path = "",
  camera_kb_path = "",
  camera_mode = "none",
  camera_frames = 0,
}

local peeking = false
local anim_t = 0.0
local stream_live = false
local last_printer_state = ""

local function configured_peek_key()
  local cfg = module.config()
  if cfg.peek_key_enabled == false then
    return nil
  end
  local k = tonumber(cfg.peek_key_index)
  if k == nil then return 13 end
  if k < 0 or k > 84 then return nil end
  return math.floor(k)
end

local function is_printing()
  local st = string.upper(tostring(status.state or ""))
  return st == "PRINTING" or st == "PAUSED" or st == "ATTENTION"
end

local function active_video_mode()
  -- Explicit hold-to-peek ALWAYS opens the sleek centered popup card!
  if peeking then
    return "popup"
  end
  if is_printing() then
    local mode = tostring(module.config().video_while_printing or "off")
    if mode == "tile" or mode == "popup" or mode == "background" then
      return mode
    end
  end
  return "off"
end

local function fmt_eta(mins)
  local m = math.floor((tonumber(mins) or 0) + 0.5)
  if m <= 0 then return "--" end
  local h = math.floor(m / 60)
  local rem = m % 60
  if h > 0 then return string.format("%dh%02dm", h, rem) end
  return string.format("%dm", rem)
end

local function publish_peek_key()
  local pk = configured_peek_key()
  if not pk then
    bus.publish("keys/providers/prusa-peek", nil, true)
    return
  end
  local col = tostring(module.config().accent or "#fa6831")
  local icon_path = (type(module.dir) == "function") and (module.dir() .. "/prusa-cam.png") or nil
  bus.publish("keys/providers/prusa-peek", {
    priority = 36,
    mode = "replace",
    rules = {
      {
        key = pk,
        label = icon_path and "" or "PRT",
        image = icon_path,
        color = col,
        bg = peeking and "#4a1c0b" or nil,
        border = col,
      },
    },
  }, true)
end

function on_init()
  local s = bus.get("modules/prusa-connect/status")
  if type(s) == "table" then
    status = s
    last_printer_state = string.upper(tostring(s.state or ""))
  end
  local st = bus.get("modules/streams/prusa-cam")
  if type(st) == "table" and st.active == true then
    stream_live = true
  end
  publish_peek_key()
end

function on_event(topic, payload)
  if topic == "modules/prusa-connect/status" and type(payload) == "table" then
    local next_state = string.upper(tostring(payload.state or ""))
    if last_printer_state == "PRINTING" and (next_state == "FINISHED" or next_state == "IDLE" or next_state == "READY") then
      local job = tostring(payload.job_name or status.job_name or "Print")
      bus.publish("ui/toast", {
        id = "prusa-complete",
        icon = "check",
        title = "Print Complete!",
        subtitle = job,
        color = "#58f29c",
        duration_ms = 6000,
      }, false)
    elseif next_state == "ERROR" and last_printer_state ~= "ERROR" then
      bus.publish("ui/toast", {
        id = "prusa-error",
        icon = "alert",
        title = "Printer Attention Required",
        subtitle = tostring(payload.name or "Prusa"),
        color = "#ff5c6c",
        duration_ms = 6000,
      }, false)
    end
    last_printer_state = next_state
    status = payload
  elseif topic == "modules/streams/prusa-cam" then
    stream_live = (type(payload) == "table" and payload.active == true)
  elseif topic == "modules/prusa-connect/peek" and type(payload) == "table" then
    local next_p = (payload.active == true)
    if next_p ~= peeking then
      peeking = next_p
      publish_peek_key()
    end
  elseif topic == "input/keys/event" and type(payload) == "table" then
    local pk = configured_peek_key()
    local ki = tonumber(payload.key_index)
    if ki == nil then ki = tonumber(payload.key) end
    if pk and ki == pk then
      local next_p = (payload.kind == "press") or (payload.pressed == true)
      if next_p ~= peeking then
        peeking = next_p
        bus.publish("modules/prusa-connect/peek", { active = peeking }, true)
        publish_peek_key()
      end
    end
  end
end

function on_tick(dt)
  anim_t = anim_t + dt
end

function on_render(ctx, stage)
  local orange = module.color("accent", "#fa6831")
  local vmode = active_video_mode()

  if stage == "background" then
    if vmode == "background" and stream_live and ctx.stream_background then
      ctx:stream_background("prusa-cam")
    end
    return
  elseif stage == "tile" then
    local r = module.slot_rect()
    if not r then return end

    local st = string.upper(tostring(status.state or "OFFLINE"))
    local prog = math.max(0, math.min(100, tonumber(status.progress) or 0))
    local noz = math.floor((tonumber(status.nozzle_temp) or 0) + 0.5)
    local noz_t = math.floor((tonumber(status.nozzle_target) or 0) + 0.5)
    local bed = math.floor((tonumber(status.bed_temp) or 0) + 0.5)
    local bed_t = math.floor((tonumber(status.bed_target) or 0) + 0.5)
    local eta = fmt_eta(status.time_remaining_min)

    local state_col = orange
    if st == "IDLE" or st == "READY" or st == "FINISHED" then
      state_col = { 0.38, 0.94, 0.62, 1.0 }
    elseif st == "OFFLINE" or st == "ERROR" then
      state_col = { 0.96, 0.38, 0.42, 0.9 }
    end

    local compact = r.w < 300
    local wide = r.w >= 620
    local pad_x = compact and 14 or 20

    -- Optional live camera directly inside the top-bar tile slot while printing!
    -- Compact telemetry header sits cleanly at the top, and video occupies the upper/middle
    -- viewing sweet-spot (keeping clear of the bottom bezel shadow).
    if vmode == "tile" and not peeking and stream_live and ctx.stream_rect then
      local pulse = (math.floor(anim_t * 3.0) % 2 == 0) and 1.0 or 0.45
      ctx:circle(r.x + 18, r.y + 20, 5, { 1.0, 0.25, 0.28, pulse })
      ctx:text(
        r.x + 30,
        r.y + 11,
        string.format("%d%%  %s", math.floor(prog + 0.5), eta ~= "--" and eta or st),
        16,
        { 0.96, 0.98, 1.0, 0.98 },
        "left"
      )
      if not compact then
        ctx:text(
          r.x + r.w - 14,
          r.y + 12,
          string.format("N %d°  B %d°", noz, bed),
          14,
          { 0.95, 0.78, 0.58, 0.92 },
          "right"
        )
      end
      local vx, vy, vw, vh = r.x + 12, r.y + 34, r.w - 24, math.max(60, r.h - 62)
      ctx:round_rect(vx - 1, vy - 1, vw + 2, vh + 2, 10, { 0.05, 0.07, 0.11, 0.9 }, 1.2, orange)
      ctx:stream_rect(vx, vy, vw, vh, "prusa-cam", 10)
      return
    end

    if compact then
      ctx:circle(r.x + 20, r.y + 24, 6, state_col)
      ctx:text(r.x + 34, r.y + 13, string.format("%d%%", math.floor(prog + 0.5)), 22, { 0.96, 0.98, 1.0, 0.98 }, "left")
      local bar_x, bar_y, bar_w, bar_h = r.x + pad_x, r.y + 50, r.w - pad_x * 2, 11
      ctx:round_rect(bar_x, bar_y, bar_w, bar_h, 5.5, { 1, 1, 1, 0.14 }, 0, { 0, 0, 0, 0 })
      if prog > 0.5 then
        ctx:round_rect(bar_x, bar_y, math.max(10, bar_w * (prog / 100.0)), bar_h, 5.5, state_col, 0, { 0, 0, 0, 0 })
      end
      ctx:text(r.x + pad_x, r.y + 76, string.format("N %d°  B %d°", noz, bed), 15, { 0.84, 0.9, 0.98, 0.92 }, "left")
      ctx:text(r.x + pad_x, r.y + 104, eta ~= "--" and ("ETA " .. eta) or st, 15, state_col, "left")
      return
    end

    -- Top-bar ergonomic safe zone: keep all telemetry comfortably above the bottom 15% bezel lip!
    ctx:circle(r.x + 26, r.y + 24, 6, state_col)
    local title_txt = wide and (string.upper(tostring(status.name or "PRUSA")) .. "  •  " .. st) or ("PRUSA  " .. st)
    ctx:text(r.x + 40, r.y + 14, title_txt, 18, state_col, "left")
    if stream_live then
      ctx:circle(r.x + r.w - 76, r.y + 24, 5, { 1.0, 0.25, 0.28, 1.0 })
      ctx:text(r.x + r.w - 20, r.y + 15, "LIVE", 15, { 1.0, 0.45, 0.45, 0.95 }, "right")
    elseif configured_peek_key() ~= nil then
      ctx:text(r.x + r.w - 20, r.y + 15, "HOLD CAM", 14, { 0.72, 0.80, 0.92, 0.65 }, "right")
    end

    local bar_x = r.x + pad_x
    local bar_y = r.y + 44
    local bar_w = r.w - pad_x * 2
    local bar_h = wide and 18 or 14
    ctx:round_rect(bar_x, bar_y, bar_w, bar_h, bar_h * 0.5, { 1, 1, 1, 0.14 }, 0, { 0, 0, 0, 0 })
    if prog > 0.5 then
      ctx:round_rect(bar_x, bar_y, math.max(12, bar_w * (prog / 100.0)), bar_h, bar_h * 0.5, state_col, 0, { 0, 0, 0, 0 })
    end
    ctx:text(bar_x, bar_y + bar_h + 10, string.format("%d%%", math.floor(prog + 0.5)), wide and 32 or 24, { 0.98, 0.99, 1.0, 0.98 }, "left")
    local job = tostring(status.job_name or "")
    if wide and #job > 0 then
      if #job > 32 then job = string.sub(job, 1, 31) .. "…" end
      ctx:text(bar_x + 104, bar_y + bar_h + 16, job, 18, { 0.82, 0.88, 0.96, 0.88 }, "left")
    end
    ctx:text(r.x + r.w - pad_x, bar_y + bar_h + 14, eta ~= "--" and ("ETA " .. eta) or tostring(status.name or ""), wide and 21 or 17, { 0.82, 0.88, 0.96, 0.88 }, "right")

    local ty = bar_y + bar_h + 48
    ctx:text(bar_x, ty, string.format("NOZ %d/%d°C", noz, noz_t), wide and 19 or 16, { 0.98, 0.74, 0.52, 0.96 }, "left")
    ctx:text(r.x + r.w - pad_x, ty, string.format("BED %d/%d°C", bed, bed_t), wide and 19 or 16, { 0.68, 0.86, 1.0, 0.96 }, "right")

  elseif stage == "world_overlay" and vmode == "popup" then
    -- Sleek centered Camera Popup card (either on Hold Peek Key or Auto-While-Printing)!
    local ox, oy, ow, oh = 360, 225, 1200, 740
    ctx:round_rect(ox, oy, ow, oh, 24, { 0.03, 0.05, 0.09, 0.94 }, 2.5, orange)

    local img_x, img_y, img_w, img_h = ox + 20, oy + 62, ow - 40, oh - 134
    ctx:round_rect(img_x, img_y, img_w, img_h, 14, { 0.07, 0.09, 0.14, 1.0 }, 1.0, { 1, 1, 1, 0.12 })

    local cam_path = tostring(status.camera_kb_path or "")
    if type(status.camera_path) == "string" and #status.camera_path > 0 and string.sub(module.dir(), 1, 5) ~= "/usr/" and string.sub(module.dir(), 1, 6) ~= "/home/" then
      cam_path = status.camera_path
    end
    local cmode = tostring(status.camera_mode or "")
    if stream_live and ctx.stream_rect then
      -- Live 20 FPS hardware-decoded H.264 right inside the popup rectangle!
      ctx:stream_rect(img_x, img_y, img_w, img_h, "prusa-cam", 14)
    elseif #cam_path > 0 and cmode ~= "none" then
      ctx:image(img_x, img_y, img_w, img_h, cam_path, { 1, 1, 1, 1 })
    elseif cmode == "live" then
      ctx:text(ox + ow * 0.5, oy + oh * 0.46, "Waiting for Buddy3D RTSP camera…", 26, { 0.92, 0.96, 1.0, 0.92 }, "center")
      ctx:text(ox + ow * 0.5, oy + oh * 0.54, "Check RTSP on your camera", 18, { 0.68, 0.78, 0.90, 0.78 }, "center")
    else
      ctx:text(ox + ow * 0.5, oy + oh * 0.44, "No live camera URL configured yet", 28, { 0.88, 0.92, 0.98, 0.9 }, "center")
      ctx:text(ox + ow * 0.5, oy + oh * 0.52, "Enable RTSP on your Buddy3D Camera and set Camera URL in Studio", 20, { 0.68, 0.78, 0.90, 0.78 }, "center")
    end

    if stream_live then
      local pulse = (math.floor(anim_t * 3.0) % 2 == 0) and 1.0 or 0.45
      ctx:circle(ox + 34, oy + 29, 7, { 1.0, 0.24, 0.26, pulse })
      ctx:text(ox + 50, oy + 18, "LIVE CAMERA  •  " .. tostring(status.name or "Prusa"), 22, orange, "left")
    else
      ctx:text(ox + 28, oy + 18, "PRINT CAMERA  •  " .. tostring(status.name or "Prusa"), 22, orange, "left")
    end
    ctx:text(ox + ow - 28, oy + 20, peeking and "Release key to close" or "Auto while printing", 18, { 0.75, 0.82, 0.92, 0.8 }, "right")

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
