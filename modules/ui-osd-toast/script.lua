-- macOS-Style OSD Toast & Meter HUD (`ui-osd-toast`, provides = ["osd_toast"])
-- Subscribes to `ui/toast` and renders two clean opinionated pill layouts:
--   1. Meter Pill (when `level` 0..1 is present: volume, mute, brightness, mic)
--   2. Status / Alert Pill (when `level` is omitted: Caps, Layer, Pomodoro, Prusa, 3rd-party alerts)
-- Publishing with the same `id` updates the pill smoothly in place and resets its fade timer!

local toasts = {} -- ordered list of active toasts (id -> replaced in place)
local attachments = nil

function on_init()
  local att = bus.get("keyboard/attachments")
  if type(att) == "table" then attachments = att end
end

local function parse_hex(hex, fallback)
  if type(hex) ~= "string" then return fallback end
  local s = hex:gsub("^#", "")
  if #s == 6 then
    local r = tonumber(s:sub(1, 2), 16)
    local g = tonumber(s:sub(3, 4), 16)
    local b = tonumber(s:sub(5, 6), 16)
    if r and g and b then return { r / 255.0, g / 255.0, b / 255.0, 1.0 } end
  end
  return fallback
end

local function draw_osd_icon(ctx, icon, cx, cy, col)
  local a = col[4] or 1.0
  local b = { math.min(1, col[1] * 0.7 + 0.3), math.min(1, col[2] * 0.7 + 0.3), math.min(1, col[3] * 0.7 + 0.3), a }
  if icon == "volume" then
    ctx:round_rect(cx - 14, cy - 6, 8, 12, 2, b, 0, { 0, 0, 0, 0 })
    ctx:line(cx - 6, cy - 6, cx + 4, cy - 13, 3.2, b)
    ctx:line(cx - 6, cy + 6, cx + 4, cy + 13, 3.2, b)
    ctx:line(cx + 4, cy - 13, cx + 4, cy + 13, 3.2, b)
    ctx:arc(cx + 5, cy, 8, -48, 48, 2.4, col)
    ctx:arc(cx + 5, cy, 14, -44, 44, 2.2, { col[1], col[2], col[3], a * 0.7 })
  elseif icon == "mute" then
    local mcol = { 1.0, 0.42, 0.46, a }
    ctx:round_rect(cx - 14, cy - 6, 8, 12, 2, mcol, 0, { 0, 0, 0, 0 })
    ctx:line(cx - 6, cy - 6, cx + 4, cy - 13, 3.2, mcol)
    ctx:line(cx - 6, cy + 6, cx + 4, cy + 13, 3.2, mcol)
    ctx:line(cx + 4, cy - 13, cx + 4, cy + 13, 3.2, mcol)
    ctx:line(cx + 9, cy - 6, cx + 19, cy + 6, 2.8, mcol)
    ctx:line(cx + 19, cy - 6, cx + 9, cy + 6, 2.8, mcol)
  elseif icon == "brightness" then
    ctx:circle(cx, cy, 7.5, b)
    for ang = 0, 315, 45 do
      local rad = math.rad(ang)
      ctx:line(cx + math.cos(rad) * 11, cy + math.sin(rad) * 11, cx + math.cos(rad) * 16, cy + math.sin(rad) * 16, 2.4, col)
    end
  elseif icon == "mic" then
    ctx:round_rect(cx - 5, cy - 13, 10, 15, 5, b, 0, { 0, 0, 0, 0 })
    ctx:arc(cx, cy - 1, 9.5, 0, 180, 2.6, col)
    ctx:line(cx, cy + 8, cx, cy + 14, 2.6, col)
  elseif icon == "check" then
    ctx:circle(cx, cy, 14, { col[1], col[2], col[3], a * 0.22 }, 2.2, col)
    ctx:line(cx - 6, cy + 1, cx - 1, cy + 6, 3.0, b)
    ctx:line(cx - 1, cy + 6, cx + 8, cy - 6, 3.0, b)
  elseif icon == "bell" then
    ctx:arc(cx, cy + 1, 10, 180, 360, 2.8, b)
    ctx:line(cx - 12, cy + 7, cx + 12, cy + 7, 2.8, b)
    ctx:circle(cx, cy + 12, 3.0, col)
  elseif icon == "play" then
    ctx:line(cx - 6, cy - 10, cx - 6, cy + 10, 3.0, b)
    ctx:line(cx - 6, cy - 10, cx + 10, cy, 3.0, b)
    ctx:line(cx - 6, cy + 10, cx + 10, cy, 3.0, b)
  else
    ctx:circle(cx, cy, 12, { col[1], col[2], col[3], a * 0.24 }, 2.4, col)
    ctx:circle(cx, cy - 4, 2.2, b)
    ctx:line(cx, cy + 1, cx, cy + 8, 2.6, b)
  end
end

function on_event(topic, payload)
  if topic == "keyboard/attachments" and type(payload) == "table" then
    attachments = payload
    return
  end
  if topic ~= "ui/toast" or type(payload) ~= "table" then return end
  local id = tostring(payload.id or ("toast-" .. tostring(#toasts + 1)))
  local dur = math.max(0.4, math.min(12.0, (tonumber(payload.duration_ms) or 1800) / 1000.0))
  local has_level = (payload.level ~= nil)
  local target_lvl = has_level and math.max(0.0, math.min(1.0, tonumber(payload.level) or 0.0)) or nil

  -- Replace in place if a toast with the same `id` is already active (zero spam stacking!)
  for _, t in ipairs(toasts) do
    if t.id == id then
      t.title = tostring(payload.title or t.title or "")
      t.message = tostring(payload.message or "")
      t.icon = tostring(payload.icon or t.icon or "info")
      t.color = payload.color or t.color
      t.value_text = payload.value_text and tostring(payload.value_text) or nil
      t.has_level = has_level
      t.target_level = target_lvl
      if not t.disp_level and target_lvl then t.disp_level = target_lvl end
      t.remaining = dur
      t.duration = dur
      -- Keep fade-in completed when updating an already visible pill
      t.age = math.max(t.age, 0.14)
      return
    end
  end

  -- Keep at most 2 simultaneous pills so the screen stays calm and uncluttered
  if #toasts >= 2 then table.remove(toasts, 1) end
  table.insert(toasts, {
    id = id,
    title = tostring(payload.title or ""),
    message = tostring(payload.message or ""),
    icon = tostring(payload.icon or "info"),
    color = payload.color,
    value_text = payload.value_text and tostring(payload.value_text) or nil,
    has_level = has_level,
    target_level = target_lvl,
    disp_level = target_lvl,
    age = 0.0,
    remaining = dur,
    duration = dur,
  })
end

function on_update(dt)
  if #toasts == 0 then return end
  local kept = {}
  for _, t in ipairs(toasts) do
    t.age = t.age + dt
    t.remaining = t.remaining - dt
    if t.has_level and t.target_level and t.disp_level then
      local diff = t.target_level - t.disp_level
      t.disp_level = t.disp_level + diff * math.min(1.0, dt * 22.0)
    end
    if t.remaining > 0 then
      table.insert(kept, t)
    end
  end
  toasts = kept
end

function on_render(ctx, stage)
  if stage ~= "world_overlay" or #toasts == 0 then return end

  local cfg = module.config()
  local pos = tostring(cfg.position or "top_bar")
  local def_col = module.color("default_accent", "#5ce1ff")

  -- Default `top_bar`: centered vertically & horizontally inside the top-bar area (y=5..261).
  -- Attachment-aware: if a physical hardware attachment (e.g. Rotary Dial / Slider) blocks the
  -- center top-bar bays, automatically relocate into the widest clear glass bay — or drop
  -- gracefully to `top_center` (y=266) if all top bays are blocked!
  local base_y = 95
  local step_y = 88
  local center_x = 960
  local max_pw = 560
  if pos == "top_center" then
    base_y = 266
  elseif pos == "bottom_center" then
    base_y = 964
    step_y = -92
  elseif pos == "top_bar" and type(attachments) == "table" then
    local blocked = attachments.blocked_rects
    local clear = attachments.clear_rects
    local default_x0, default_x1 = 960 - 270, 960 + 270
    local hits_blocked = false
    if type(blocked) == "table" then
      for _, br in ipairs(blocked) do
        local bx0 = tonumber(br.x) or 0
        local bx1 = bx0 + (tonumber(br.w) or 0)
        if default_x0 < bx1 and default_x1 > bx0 then
          hits_blocked = true
          break
        end
      end
    end
    if hits_blocked then
      local best_cx, best_w = nil, 0
      if type(clear) == "table" then
        for _, cr in ipairs(clear) do
          local cw = tonumber(cr.w) or 0
          local cx = (tonumber(cr.x) or 0) + cw * 0.5
          if cw > best_w then
            best_w = cw
            best_cx = cx
          end
        end
      end
      if best_cx and best_w >= 400 then
        center_x = best_cx
        max_pw = math.min(520, math.floor(best_w - 28))
      else
        base_y = 266
      end
    end
  end

  for idx, t in ipairs(toasts) do
    local fade_in = math.min(1.0, t.age / 0.12)
    local fade_out = math.min(1.0, t.remaining / 0.26)
    local alpha = math.max(0.0, math.min(1.0, fade_in * fade_out))
    if alpha > 0.02 then
      local c = parse_hex(t.color, def_col)
      local ac = { c[1], c[2], c[3], alpha }
      local white = { 0.96, 0.98, 1.0, alpha * 0.98 }
      local sub_col = { 0.76, 0.84, 0.94, alpha * 0.86 }
      local cy_slide = (1.0 - fade_in) * 8.0
      local py = base_y + (idx - 1) * step_y - cy_slide

      if t.has_level then
        -- macOS-grade Meter Pill (Volume, Brightness, Mic Gain)
        local pw, ph = math.min(520, max_pw), 76
        local px = center_x - pw * 0.5
        ctx:round_rect(px, py, pw, ph, 22, { 0.04, 0.06, 0.10, alpha * 0.90 }, 1.6, { c[1], c[2], c[3], alpha * 0.55 })
        draw_osd_icon(ctx, t.icon, px + 38, py + ph * 0.5, ac)

        local lvl = math.max(0.0, math.min(1.0, t.disp_level or 0.0))
        local val_s = t.value_text or string.format("%d%%", math.floor(lvl * 100 + 0.5))
        local bar_x = px + 76
        local bar_w = pw - 168
        local bar_y = py + (#t.title > 0 and 44 or 31)

        if #t.title > 0 then
          ctx:text(bar_x, py + 14, t.title, 16, sub_col, "left")
        end
        ctx:round_rect(bar_x, bar_y, bar_w, 12, 6, { 1.0, 1.0, 1.0, alpha * 0.14 }, 0, { 0, 0, 0, 0 })
        if lvl > 0.005 then
          ctx:round_rect(bar_x, bar_y, math.max(12, bar_w * lvl), 12, 6, ac, 0, { 0, 0, 0, 0 })
        end
        ctx:text(px + pw - 22, py + 24, val_s, 22, white, "right")
      else
        -- Clean Status / Alert Pill (Caps, Layer, Pomodoro, Prusa, 3rd-party notifications)
        local has_msg = (#t.message > 0)
        local pw = math.min(has_msg and 560 or 380, max_pw)
        local ph = has_msg and 78 or 64
        local px = center_x - pw * 0.5
        ctx:round_rect(px, py, pw, ph, 20, { 0.04, 0.06, 0.10, alpha * 0.90 }, 1.6, { c[1], c[2], c[3], alpha * 0.62 })
        draw_osd_icon(ctx, t.icon, px + 36, py + ph * 0.5, ac)
        if has_msg then
          ctx:text(px + 68, py + 13, t.title, 20, white, "left")
          ctx:text(px + 68, py + 42, t.message, 16, sub_col, "left")
        else
          ctx:text(px + 68, py + 20, t.title, 22, white, "left")
        end
      end
    end
  end
end
