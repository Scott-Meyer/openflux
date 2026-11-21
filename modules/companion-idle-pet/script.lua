-- OpenFlux v2 Architectural North Star: The Idle Pet Module!
-- Demonstrates:
-- 1. Physical keyboard geometry (`keyboard.layout().keys` + `module.slot_rect()`)
-- 2. Rendering inside its top-bar slot (`tile`) AND spilling out onto the global canvas (`world_overlay`)
-- 3. Cross-module composition via `fx/surface/impulse` (paw landings trigger 3D Water Ripples & Particles
--    without either module knowing the other exists!)
-- 4. Reacting to upper-row key presses (reaching a paw down from the top slot!) and startling at nearby ripples.

local layout = nil
local mode = "slot" -- "slot" | "swipe" | "hopping" | "roaming"
local pos_x, pos_y = 1650.0, 120.0
local home_x, home_y = 1650.0, 120.0
local start_x, start_y = 1650.0, 120.0
local target_x, target_y = 1650.0, 120.0
local hop_t = 0.0
local hop_dur = 0.45
local idle_timer = 0.0
local roam_pause = 0.0
local paw_timer = 0.0
local swipe_target_x, swipe_target_y = 1650.0, 340.0
local startle_timer = 0.0
local anim_t = 0.0
local current_key_index = nil
local hop_count = 0

function on_init()
  layout = keyboard.layout()
  local sr = module.slot_rect()
  if sr then
    home_x = sr.x + sr.w * 0.5
    home_y = sr.y + sr.h * 0.58
    pos_x, pos_y = home_x, home_y
  end
end

local function begin_hop_to(tx, ty, key_idx)
  start_x, start_y = pos_x, pos_y
  target_x, target_y = tx, ty
  current_key_index = key_idx
  hop_t = 0.0
  hop_dur = 0.42
  mode = "hopping"
end

local function pick_next_key()
  if not layout or not layout.keys or #layout.keys == 0 then return nil end
  hop_count = hop_count + 1
  -- Pick a lively central/upper keycap to hop onto.
  local idx = 22 + ((hop_count * 13) % math.max(1, #layout.keys - 30))
  return layout.keys[idx]
end

local function emit_paw_impulse(x, y, force)
  bus.publish("fx/surface/impulse", {
    x = x,
    y = y,
    radius = 64.0,
    force = force or 0.95,
    kind = "paw",
    source = "module:companion-idle-pet",
    key_index = current_key_index,
  }, false)
end

function on_event(topic, payload, meta)
  if topic == "modules/companion-idle-pet/command" and type(payload) == "table" then
    if payload.action == "roam" and layout and layout.keys then
      local tk = payload.target_key or 38
      local k = layout.keys[tk + 1] or layout.keys[35]
      if k then
        begin_hop_to(k.center_x, k.center_y, k.key_index)
      end
    elseif payload.action == "home" then
      begin_hop_to(home_x, home_y, nil)
    end
  elseif topic == "input/keys/event" and payload.kind == "press" then
    idle_timer = 0.0
    local dx = (payload.x_px or 0) - pos_x
    local dy = (payload.y_px or 0) - pos_y
    local dist = math.sqrt(dx * dx + dy * dy)

    if mode == "slot" and (payload.row or 9) <= 1 and math.abs((payload.x_px or 0) - home_x) < 420 then
      -- Reach a playful paw down from the top slot toward upper-row typing!
      mode = "swipe"
      paw_timer = 0.55
      swipe_target_x = payload.x_px
      swipe_target_y = payload.y_px
    elseif (mode == "roaming" or mode == "hopping") and dist < 240 then
      -- Startled by a nearby keypress while playing on the keys!
      startle_timer = 0.65
      local k = pick_next_key()
      if k then begin_hop_to(k.center_x, k.center_y, k.key_index) end
    end
  elseif topic == "fx/surface/impulse" then
    -- Ignore our own paw impulses!
    if payload.source == "module:companion-idle-pet" then return end
    local dx = (payload.x or 0) - pos_x
    local dy = (payload.y or 0) - pos_y
    if math.sqrt(dx * dx + dy * dy) < 260 and (payload.force or 0) >= 0.7 then
      startle_timer = 0.55
    end
  end
end

function on_update(dt)
  anim_t = anim_t + dt
  if startle_timer > 0 then startle_timer = math.max(0, startle_timer - dt) end

  local idle_limit = tonumber(module.config().idle_seconds) or 15.0

  if mode == "slot" then
    idle_timer = idle_timer + dt
    pos_x, pos_y = home_x, home_y
    if idle_timer >= idle_limit then
      local k = pick_next_key()
      if k then begin_hop_to(k.center_x, k.center_y, k.key_index) end
    end
  elseif mode == "swipe" then
    paw_timer = paw_timer - dt
    if paw_timer <= 0 then
      mode = "slot"
    end
  elseif mode == "hopping" then
    hop_t = hop_t + dt / hop_dur
    if hop_t >= 1.0 then
      hop_t = 1.0
      pos_x, pos_y = target_x, target_y
      if current_key_index ~= nil then
        mode = "roaming"
        roam_pause = 1.15
        -- Paw lands on the keycap -> send real surface impulse onto the shared bus!
        emit_paw_impulse(pos_x, pos_y, 1.05)
      else
        mode = "slot"
      end
    else
      local s = hop_t * hop_t * (3.0 - 2.0 * hop_t)
      local arc = math.sin(hop_t * math.pi) * 62.0
      pos_x = start_x + (target_x - start_x) * s
      pos_y = start_y + (target_y - start_y) * s - arc
    end
  elseif mode == "roaming" then
    roam_pause = roam_pause - dt
    if roam_pause <= 0 then
      if hop_count % 5 == 0 then
        begin_hop_to(home_x, home_y, nil)
      else
        local k = pick_next_key()
        if k then begin_hop_to(k.center_x, k.center_y, k.key_index) end
      end
    end
  end
end

local function draw_cat(ctx, x, y, scale, fur, startled)
  local bob = math.sin(anim_t * 5.2) * 3.0 * scale
  local cy = y + bob
  local shadow = { 0.0, 0.0, 0.0, 0.35 }
  local belly = { 1.0, 0.94, 0.86, 0.95 }
  local ear_in = { 1.0, 0.62, 0.68, 0.95 }
  local dark = { 0.11, 0.12, 0.16, 1.0 }

  -- Soft ground shadow
  ctx:ellipse(x, y + 32 * scale, 34 * scale, 10 * scale, shadow)

  -- Wagging tail
  local tail_wag = math.sin(anim_t * 6.5) * 14 * scale
  ctx:line(x + 24 * scale, cy + 12 * scale, x + 46 * scale, cy - 8 * scale + tail_wag, 9 * scale, fur)

  -- Body
  ctx:ellipse(x, cy + 10 * scale, 30 * scale, 22 * scale, fur)
  ctx:ellipse(x, cy + 15 * scale, 18 * scale, 13 * scale, belly)

  -- Paws
  ctx:circle(x - 14 * scale, cy + 28 * scale, 8 * scale, belly)
  ctx:circle(x + 14 * scale, cy + 28 * scale, 8 * scale, belly)

  -- Head
  local hy = cy - 16 * scale
  ctx:circle(x, hy, 22 * scale, fur)

  -- Pointed ears (perked taller when startled)
  local ear_lift = startled and 8 * scale or 0
  ctx:circle(x - 15 * scale, hy - 18 * scale - ear_lift, 9 * scale, fur)
  ctx:circle(x + 15 * scale, hy - 18 * scale - ear_lift, 9 * scale, fur)
  ctx:circle(x - 15 * scale, hy - 17 * scale - ear_lift, 5 * scale, ear_in)
  ctx:circle(x + 15 * scale, hy - 17 * scale - ear_lift, 5 * scale, ear_in)

  -- Eyes (occasional blink, wide open when startled)
  local blinking = (not startled) and (math.floor(anim_t * 10) % 45 == 0)
  if blinking then
    ctx:line(x - 11 * scale, hy - 2 * scale, x - 4 * scale, hy - 2 * scale, 2.5 * scale, dark)
    ctx:line(x + 4 * scale, hy - 2 * scale, x + 11 * scale, hy - 2 * scale, 2.5 * scale, dark)
  else
    local er = startled and 5.2 * scale or 3.8 * scale
    ctx:circle(x - 8 * scale, hy - 2 * scale, er, dark)
    ctx:circle(x + 8 * scale, hy - 2 * scale, er, dark)
    ctx:circle(x - 6.5 * scale, hy - 3.5 * scale, 1.4 * scale, { 1, 1, 1, 1 })
    ctx:circle(x + 9.5 * scale, hy - 3.5 * scale, 1.4 * scale, { 1, 1, 1, 1 })
  end

  -- Cute pink nose
  ctx:circle(x, hy + 5 * scale, 2.6 * scale, ear_in)

  if startled then
    ctx:text(x + 28 * scale, hy - 34 * scale, "!", 28 * scale, { 1.0, 0.92, 0.3, 1.0 }, "center")
  end
end

function on_render(ctx, stage)
  local fur = module.color("fur_color", "#ffb86c")
  local pet_name = tostring(module.config().pet_name or "Mochi")
  local sr = module.slot_rect()

  if stage == "tile" and sr then
    ctx:clip(sr.x, sr.y, sr.w, sr.h)
    ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.72 }, 1.5, { fur[1], fur[2], fur[3], 0.48 })
    -- Cozy cushion inside the pet's home slot
    ctx:ellipse(home_x, sr.y + sr.h - 24, 72, 16, { 0.22, 0.16, 0.30, 0.85 })
    ctx:text(sr.x + 18, sr.y + 14, pet_name, 22, { 1.0, 0.92, 0.82, 0.95 }, "left")
    local status = (mode == "slot") and "napping" or (mode == "swipe" and "swiping!" or "playing!")
    ctx:text(sr.x + sr.w - 18, sr.y + 16, status, 18, { 0.65, 0.85, 1.0, 0.85 }, "right")

    if mode == "slot" or mode == "swipe" then
      draw_cat(ctx, home_x, home_y, 1.0, fur, startle_timer > 0)
    end
    ctx:clear_clip()
  elseif stage == "world_overlay" then
    if mode == "hopping" or mode == "roaming" then
      draw_cat(ctx, pos_x, pos_y, 1.15, fur, startle_timer > 0)
    elseif mode == "swipe" and sr then
      -- Reach a playful paw down out of the top-bar slot onto the upper key row!
      local reach = math.sin((paw_timer / 0.55) * math.pi)
      local px = home_x + (swipe_target_x - home_x) * reach * 0.75
      local py = (sr.y + sr.h) + (swipe_target_y - (sr.y + sr.h)) * reach
      ctx:line(home_x, sr.y + sr.h - 10, px, py, 16, fur)
      ctx:circle(px, py, 14, { 1.0, 0.94, 0.86, 0.96 })
      ctx:circle(px, py + 2, 6, { 1.0, 0.62, 0.68, 0.95 })
    end
  end
end
