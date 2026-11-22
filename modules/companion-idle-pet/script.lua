-- Mochi the Cat — Mario-Style Platformer Cat Companion!
-- Walks and runs directly ON the bottom divider ledge of each physical key row,
-- shifts down/up one row at a time with crisp Mario jump arcs, and runs back home
-- row-by-row along the keys before climbing into her top-bar slot (never flies across the board!).

local layout = nil
local rows = {} -- rows[0..5] = { y_bottom, min_x, max_x, keys = {...} }
local mode = "slot" -- "slot" | "swipe" | "walk" | "sit" | "jump"
local cur_row = 0
local pos_x, pos_y = 1650.0, 120.0
local home_x, home_y = 1650.0, 120.0
local facing = -1 -- -1 left, +1 right
local walk_speed = 205.0
local walk_timer = 0.0
local step_accum = 0.0
local sit_timer = 0.0
local idle_timer = 0.0
local paw_timer = 0.0
local swipe_target_x, swipe_target_y = 1650.0, 340.0
local startle_timer = 0.0
local anim_t = 0.0
local returning_home = false

-- Short physical Mario jump between adjacent ledges
local j_sx, j_sy = 0.0, 0.0
local j_tx, j_ty = 0.0, 0.0
local j_t, j_dur = 0.0, 0.34
local j_arc = 52.0
local j_dest_row = 0
local j_into_slot = false
local action_count = 0

-- Paws are drawn at `pos_y + 25 * scale` (scale = 1.0 -> +25px), so standing ON a row's
-- bottom divider line `rr.y_bottom` means `pos_y = rr.y_bottom - 25`.
local PAW_FOOT_OFFSET = 25.0

local function row_stand_y(row_idx)
  local rr = rows[row_idx] or rows[0]
  if not rr then return 340.0 end
  return rr.y_bottom - PAW_FOOT_OFFSET
end

local function build_row_platforms()
  rows = {}
  if not layout or not layout.keys then return end
  for _, k in ipairs(layout.keys) do
    local r = k.row or 0
    local y_bot = k.rect.y + k.rect.h
    if not rows[r] then
      rows[r] = { y_bottom = y_bot, min_x = k.rect.x + 28, max_x = k.rect.x + k.rect.w - 28, keys = {} }
    end
    local rr = rows[r]
    if y_bot > rr.y_bottom then rr.y_bottom = y_bot end
    if k.rect.x + 28 < rr.min_x then rr.min_x = k.rect.x + 28 end
    if k.rect.x + k.rect.w - 28 > rr.max_x then rr.max_x = k.rect.x + k.rect.w - 28 end
    table.insert(rr.keys, k)
  end
end

function on_init()
  layout = keyboard.layout()
  build_row_platforms()
  local sr = module.slot_rect()
  if sr then
    home_x = sr.x + sr.w * 0.5
    home_y = sr.y + sr.h * 0.64
    pos_x, pos_y = home_x, home_y
  end
end

local function emit_paw_impulse(x, y, force, radius)
  bus.publish("fx/surface/impulse", {
    x = x,
    y = y,
    radius = radius or 48.0,
    force = force or 0.72,
    kind = "paw",
    source = "module:companion-idle-pet",
  }, false)
end

local function start_jump(tx, ty, dest_row, arc_h, into_slot)
  j_sx, j_sy = pos_x, pos_y
  j_tx, j_ty = tx, ty
  j_dest_row = dest_row
  j_arc = arc_h or 52.0
  j_t = 0.0
  local dist = math.sqrt((tx - pos_x) ^ 2 + (ty - pos_y) ^ 2)
  j_dur = math.max(0.24, math.min(0.42, 0.22 + dist / 1100.0))
  j_into_slot = into_slot == true
  if tx > pos_x + 4 then facing = 1 elseif tx < pos_x - 4 then facing = -1 end
  mode = "jump"
end

-- Step only to an adjacent row (or same row) so Mochi never teleports/flies across the board!
local function hop_to_row(row_idx, target_x)
  local clamped_row = math.max(0, math.min(5, row_idx))
  local rr = rows[clamped_row] or rows[0]
  if not rr then return end
  local x = math.max(rr.min_x + 36, math.min(rr.max_x - 36, target_x or pos_x))
  local arc = (clamped_row < cur_row) and 58.0 or 40.0
  start_jump(x, row_stand_y(clamped_row), clamped_row, arc, false)
end

function on_event(topic, payload)
  if topic == "modules/companion-idle-pet/command" and type(payload) == "table" then
    if payload.action == "roam" then
      returning_home = false
      if mode == "slot" then
        -- Drop straight down from her slot onto Row 0 right underneath her box!
        hop_to_row(0, home_x)
      elseif payload.target_key and layout and layout.keys and layout.keys[payload.target_key + 1] then
        local k = layout.keys[payload.target_key + 1]
        hop_to_row(k.row or 1, k.center_x)
      end
    elseif payload.action == "home" then
      if mode ~= "slot" then
        returning_home = true
        if mode == "sit" then mode = "walk" end
      end
    end
  elseif topic == "input/keys/event" and payload.kind == "press" then
    idle_timer = 0.0
    local dx = (payload.x_px or 0) - pos_x
    local dy = (payload.y_px or 0) - (pos_y + PAW_FOOT_OFFSET)
    local dist = math.sqrt(dx * dx + dy * dy)

    if mode == "slot" and (payload.row or 9) <= 1 and math.abs((payload.x_px or 0) - home_x) < 460 then
      mode = "swipe"
      paw_timer = 0.52
      swipe_target_x = payload.x_px
      swipe_target_y = payload.y_px
    elseif (mode == "walk" or mode == "sit") and dist < 195 then
      startle_timer = 0.50
      local hop_dir = (dx >= 0) and -1 or 1
      facing = hop_dir
      local next_row = (cur_row < 5) and (cur_row + 1) or (cur_row - 1)
      hop_to_row(next_row, pos_x + hop_dir * 140)
    end
  elseif topic == "fx/surface/impulse" then
    if payload.source == "module:companion-idle-pet" then return end
    local dx = (payload.x or 0) - pos_x
    local dy = (payload.y or 0) - (pos_y + PAW_FOOT_OFFSET)
    if math.sqrt(dx * dx + dy * dy) < 210 and (payload.force or 0) >= 0.75 then
      startle_timer = 0.45
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
    returning_home = false
    if idle_timer >= idle_limit then
      facing = -1
      -- Short hop straight down onto Row 0 right underneath her home box!
      hop_to_row(0, home_x - 40)
    end
  elseif mode == "swipe" then
    paw_timer = paw_timer - dt
    if paw_timer <= 0 then mode = "slot" end
  elseif mode == "jump" then
    j_t = j_t + dt / j_dur
    if j_t >= 1.0 then
      pos_x, pos_y = j_tx, j_ty
      if j_into_slot then
        mode = "slot"
        returning_home = false
        idle_timer = 0.0
      else
        cur_row = j_dest_row
        emit_paw_impulse(pos_x, pos_y + PAW_FOOT_OFFSET, 0.85, 52.0)
        mode = "walk"
        walk_timer = returning_home and 0.55 or (1.5 + ((action_count * 37) % 22) * 0.1)
        action_count = action_count + 1
      end
    else
      local s = j_t
      local arc = math.sin(s * math.pi) * j_arc
      pos_x = j_sx + (j_tx - j_sx) * s
      pos_y = j_sy + (j_ty - j_sy) * s - arc
    end
  elseif mode == "walk" then
    local rr = rows[cur_row] or rows[0]
    if not rr then mode = "slot"; return end

    -- When returning home: run along the current row toward home_x, hopping UP one row at a time,
    -- then climb straight up into her top-bar slot ONLY once she reaches directly below it on Row 0!
    if returning_home then
      local dx_home = home_x - pos_x
      if math.abs(dx_home) > 22.0 then
        facing = (dx_home > 0) and 1 or -1
        local step = math.min(math.abs(dx_home), walk_speed * 1.25 * dt)
        pos_x = pos_x + facing * step
      end
      pos_y = row_stand_y(cur_row)
      walk_timer = walk_timer - dt
      step_accum = step_accum + walk_speed * 1.25 * dt
      if step_accum >= 150.0 then
        step_accum = 0.0
        emit_paw_impulse(pos_x, pos_y + PAW_FOOT_OFFSET, 0.30, 34.0)
      end

      if cur_row > 0 and (walk_timer <= 0 or math.abs(dx_home) < 90.0) then
        -- Mario hop UP exactly one row closer to home!
        local next_x = pos_x + facing * math.min(95.0, math.abs(dx_home))
        hop_to_row(cur_row - 1, next_x)
        return
      elseif cur_row == 0 and math.abs(home_x - pos_x) <= 28.0 then
        -- Right underneath her slot on Row 0 -> short cute hop straight up into bed!
        start_jump(home_x, home_y, 0, 48.0, true)
        return
      end
      return
    end

    -- Normal Mario-style roaming along the bottom divider ledge of `cur_row`
    pos_x = pos_x + facing * walk_speed * dt
    pos_y = row_stand_y(cur_row)
    walk_timer = walk_timer - dt
    step_accum = step_accum + walk_speed * dt

    if step_accum >= 165.0 then
      step_accum = 0.0
      emit_paw_impulse(pos_x, pos_y + PAW_FOOT_OFFSET, 0.32, 34.0)
    end

    if pos_x <= rr.min_x + 30 then
      pos_x = rr.min_x + 30
      facing = 1
      if cur_row < 5 and (action_count % 2 == 0) then
        hop_to_row(cur_row + 1, pos_x + 85)
        return
      end
    elseif pos_x >= rr.max_x - 30 then
      pos_x = rr.max_x - 30
      facing = -1
      if cur_row < 5 and (action_count % 2 == 1) then
        hop_to_row(cur_row + 1, pos_x - 85)
        return
      end
    end

    if walk_timer <= 0 then
      action_count = action_count + 1
      local choice = action_count % 6
      if choice == 0 and action_count >= 12 then
        -- Start running & climbing home row-by-row!
        returning_home = true
        walk_timer = 0.45
      elseif choice == 1 or choice == 3 then
        -- Mario shift DOWN one row!
        local next_row = (cur_row < 5) and (cur_row + 1) or (cur_row - 1)
        hop_to_row(next_row, pos_x + facing * 115)
      elseif choice == 2 then
        -- Mario hop UP one row!
        local next_row = (cur_row > 0) and (cur_row - 1) or 1
        hop_to_row(next_row, pos_x + facing * 100)
      else
        mode = "sit"
        sit_timer = 1.05
        emit_paw_impulse(pos_x, pos_y + PAW_FOOT_OFFSET, 0.65, 44.0)
      end
    end
  elseif mode == "sit" then
    pos_y = row_stand_y(cur_row)
    sit_timer = sit_timer - dt
    if sit_timer <= 0 then
      if (action_count % 3) == 0 then facing = -facing end
      mode = "walk"
      walk_timer = 1.8 + ((action_count * 23) % 18) * 0.1
    end
  end
end

local function draw_cat_ear(ctx, bx, by, tip_x, tip_y, base_w, fur, inner)
  ctx:line(bx - base_w * 0.45, by, tip_x, tip_y, base_w * 0.48, fur)
  ctx:line(bx + base_w * 0.45, by, tip_x, tip_y, base_w * 0.48, fur)
  ctx:line(bx, by, tip_x, tip_y, base_w * 0.56, fur)
  ctx:line(bx, by - 1, bx + (tip_x - bx) * 0.68, by + (tip_y - by) * 0.68, base_w * 0.28, inner)
end

local function draw_cat(ctx, x, y, sc, fur, dir, pose, startled)
  local stripe = { fur[1] * 0.68, fur[2] * 0.62, fur[3] * 0.58, 0.95 }
  local cream = { 1.0, 0.96, 0.90, 0.98 }
  local pink = { 1.0, 0.62, 0.70, 0.96 }
  local dark = { 0.08, 0.10, 0.13, 1.0 }
  local eye_col = { 0.42, 0.95, 0.62, 1.0 }
  local shadow = { 0.0, 0.0, 0.0, 0.36 }

  if pose == "sleep" then
    local breathe = math.sin(anim_t * 2.2) * 1.6 * sc
    ctx:ellipse(x, y + 16 * sc, 38 * sc, 10 * sc, shadow)
    ctx:arc(x + 2 * sc, y + 6 * sc, 26 * sc, 10, 155, 8 * sc, fur)
    ctx:ellipse(x - 4 * sc, y + 2 * sc - breathe * 0.4, 30 * sc, 19 * sc + breathe * 0.4, fur)
    ctx:line(x - 14 * sc, y - 12 * sc, x - 12 * sc, y - 2 * sc, 4 * sc, stripe)
    ctx:line(x - 4 * sc, y - 14 * sc, x - 2 * sc, y - 3 * sc, 4 * sc, stripe)
    local hx = x + 16 * sc
    local hy = y - 2 * sc
    draw_cat_ear(ctx, hx - 10 * sc, hy - 12 * sc, hx - 15 * sc, hy - 26 * sc, 11 * sc, fur, pink)
    draw_cat_ear(ctx, hx + 8 * sc, hy - 12 * sc, hx + 13 * sc, hy - 26 * sc, 11 * sc, fur, pink)
    ctx:ellipse(hx, hy, 17 * sc, 14 * sc, fur)
    ctx:ellipse(hx, hy + 4 * sc, 9 * sc, 6 * sc, cream)
    ctx:arc(hx - 6 * sc, hy - 1 * sc, 4 * sc, 15, 165, 2.2 * sc, dark)
    ctx:arc(hx + 6 * sc, hy - 1 * sc, 4 * sc, 15, 165, 2.2 * sc, dark)
    ctx:circle(hx, hy + 3 * sc, 2.2 * sc, pink)
    local zy = (anim_t * 14) % 18
    ctx:text(hx + 22 * sc, hy - 18 * sc - zy, "z", 14 * sc, { 0.85, 0.92, 1.0, 0.75 }, "center")
    return
  end

  if pose == "walk" or pose == "jump" then
    local stride = (pose == "walk") and math.sin(anim_t * 16.0) or 0.95
    local bob = (pose == "walk") and math.abs(math.sin(anim_t * 16.0)) * -3.5 * sc or 0.0
    local cy = y + bob

    if pose == "walk" then
      ctx:ellipse(x, y + 25 * sc, 30 * sc, 5 * sc, shadow)
    end

    local wag = math.sin(anim_t * 8.5) * 10 * sc
    local tx0 = x - dir * 24 * sc
    local ty0 = cy + 2 * sc
    local tx1 = x - dir * 38 * sc
    local ty1 = cy - 12 * sc + wag * 0.5
    local tx2 = x - dir * 44 * sc
    local ty2 = cy - 26 * sc + wag
    ctx:line(tx0, ty0, tx1, ty1, 7.5 * sc, fur)
    ctx:line(tx1, ty1, tx2, ty2, 6.5 * sc, fur)
    ctx:circle(tx2, ty2, 3.6 * sc, cream)

    local leg_a = stride * 10 * sc
    local leg_b = -stride * 10 * sc
    ctx:line(x - dir * 15 * sc, cy + 10 * sc, x - dir * 18 * sc + leg_b, cy + 23 * sc, 7 * sc, stripe)
    ctx:circle(x - dir * 18 * sc + leg_b, cy + 24 * sc, 4.2 * sc, cream)
    ctx:line(x + dir * 14 * sc, cy + 10 * sc, x + dir * 16 * sc + leg_a, cy + 23 * sc, 7 * sc, stripe)
    ctx:circle(x + dir * 16 * sc + leg_a, cy + 24 * sc, 4.2 * sc, cream)

    ctx:ellipse(x, cy + 5 * sc, 27 * sc, 14 * sc, fur)
    ctx:ellipse(x + dir * 13 * sc, cy + 8 * sc, 11 * sc, 9 * sc, cream)
    ctx:line(x - dir * 8 * sc, cy - 8 * sc, x - dir * 6 * sc, cy + 2 * sc, 3.8 * sc, stripe)
    ctx:line(x + dir * 1 * sc, cy - 8 * sc, x + dir * 2 * sc, cy + 2 * sc, 3.8 * sc, stripe)

    ctx:line(x - dir * 11 * sc, cy + 11 * sc, x - dir * 10 * sc + leg_a, cy + 24 * sc, 7.5 * sc, fur)
    ctx:circle(x - dir * 10 * sc + leg_a, cy + 25 * sc, 4.5 * sc, cream)
    ctx:line(x + dir * 17 * sc, cy + 10 * sc, x + dir * 18 * sc + leg_b, cy + 24 * sc, 7.5 * sc, fur)
    ctx:circle(x + dir * 18 * sc + leg_b, cy + 25 * sc, 4.5 * sc, cream)

    local hx = x + dir * 23 * sc
    local hy = cy - 11 * sc
    local ear_lift = startled and 6 * sc or 0
    draw_cat_ear(ctx, hx - dir * 8 * sc, hy - 10 * sc, hx - dir * 13 * sc, hy - 27 * sc - ear_lift, 11 * sc, fur, pink)
    draw_cat_ear(ctx, hx + dir * 6 * sc, hy - 10 * sc, hx + dir * 10 * sc, hy - 26 * sc - ear_lift, 11 * sc, fur, pink)
    ctx:ellipse(hx, hy, 16.5 * sc, 14.5 * sc, fur)
    ctx:ellipse(hx + dir * 6 * sc, hy + 4 * sc, 8 * sc, 6 * sc, cream)

    local ex = hx + dir * 7 * sc
    local ey = hy - 2 * sc
    ctx:ellipse(ex, ey, 4.4 * sc, 4.8 * sc, eye_col)
    ctx:ellipse(ex + dir * 1.2 * sc, ey, startled and 2.6 * sc or 1.6 * sc, 3.8 * sc, dark)
    ctx:circle(ex + dir * 1.5 * sc, ey - 1.5 * sc, 1.2 * sc, { 1, 1, 1, 1 })

    local nx = hx + dir * 14 * sc
    local ny = hy + 3 * sc
    ctx:circle(nx, ny, 2.5 * sc, pink)
    ctx:line(nx - dir * 3 * sc, ny + 1 * sc, nx + dir * 11 * sc, ny - 2 * sc, 1.3 * sc, cream)
    ctx:line(nx - dir * 3 * sc, ny + 3 * sc, nx + dir * 12 * sc, ny + 3 * sc, 1.3 * sc, cream)
    ctx:line(nx - dir * 3 * sc, ny + 5 * sc, nx + dir * 10 * sc, ny + 7 * sc, 1.3 * sc, cream)

    if startled then
      ctx:text(hx, hy - 34 * sc, "!", 26 * sc, { 1.0, 0.92, 0.3, 1.0 }, "center")
    end
    return
  end

  local cy = y + math.sin(anim_t * 4.0) * 1.5 * sc
  ctx:ellipse(x, y + 25 * sc, 28 * sc, 6 * sc, shadow)
  local wag = math.sin(anim_t * 5.0) * 12 * sc
  ctx:line(x + dir * 18 * sc, cy + 16 * sc, x + dir * 38 * sc, cy - 4 * sc + wag, 7.5 * sc, fur)
  ctx:circle(x + dir * 38 * sc, cy - 4 * sc + wag, 3.8 * sc, cream)
  ctx:ellipse(x, cy + 8 * sc, 20 * sc, 19 * sc, fur)
  ctx:ellipse(x, cy + 12 * sc, 12 * sc, 12 * sc, cream)
  ctx:circle(x - 9 * sc, cy + 24 * sc, 5.8 * sc, cream)
  ctx:circle(x + 9 * sc, cy + 24 * sc, 5.8 * sc, cream)

  local hy = cy - 15 * sc
  draw_cat_ear(ctx, x - 11 * sc, hy - 9 * sc, x - 18 * sc, hy - 28 * sc, 12 * sc, fur, pink)
  draw_cat_ear(ctx, x + 11 * sc, hy - 9 * sc, x + 18 * sc, hy - 28 * sc, 12 * sc, fur, pink)
  ctx:ellipse(x, hy, 18 * sc, 15.5 * sc, fur)
  ctx:ellipse(x, hy + 5 * sc, 10 * sc, 6.5 * sc, cream)
  ctx:line(x - 4 * sc, hy - 13 * sc, x - 3 * sc, hy - 7 * sc, 2.2 * sc, stripe)
  ctx:line(x, hy - 14 * sc, x, hy - 7 * sc, 2.2 * sc, stripe)
  ctx:line(x + 4 * sc, hy - 13 * sc, x + 3 * sc, hy - 7 * sc, 2.2 * sc, stripe)
  ctx:ellipse(x - 7 * sc, hy - 1 * sc, 4.2 * sc, 4.6 * sc, eye_col)
  ctx:ellipse(x + 7 * sc, hy - 1 * sc, 4.2 * sc, 4.6 * sc, eye_col)
  ctx:ellipse(x - 7 * sc, hy - 1 * sc, 1.7 * sc, 3.6 * sc, dark)
  ctx:ellipse(x + 7 * sc, hy - 1 * sc, 1.7 * sc, 3.6 * sc, dark)
  ctx:circle(x - 5.5 * sc, hy - 2.5 * sc, 1.2 * sc, { 1, 1, 1, 1 })
  ctx:circle(x + 8.5 * sc, hy - 2.5 * sc, 1.2 * sc, { 1, 1, 1, 1 })
  ctx:circle(x, hy + 4 * sc, 2.5 * sc, pink)
  ctx:line(x - 12 * sc, hy + 3 * sc, x - 26 * sc, hy + 1 * sc, 1.3 * sc, cream)
  ctx:line(x - 12 * sc, hy + 6 * sc, x - 26 * sc, hy + 7 * sc, 1.3 * sc, cream)
  ctx:line(x + 12 * sc, hy + 3 * sc, x + 26 * sc, hy + 1 * sc, 1.3 * sc, cream)
  ctx:line(x + 12 * sc, hy + 6 * sc, x + 26 * sc, hy + 7 * sc, 1.3 * sc, cream)
end

function on_render(ctx, stage)
  local fur = module.color("fur_color", "#ffb86c")
  local pet_name = tostring(module.config().pet_name or "Mochi")
  local sr = module.slot_rect()

  if stage == "tile" and sr then
    ctx:clip(sr.x, sr.y, sr.w, sr.h)
    ctx:round_rect(sr.x + 2, sr.y + 2, sr.w - 4, sr.h - 4, 18, { 0.05, 0.07, 0.12, 0.72 }, 1.5, { fur[1], fur[2], fur[3], 0.48 })
    ctx:ellipse(home_x, sr.y + sr.h - 20, 74, 15, { 0.22, 0.16, 0.30, 0.85 })
    ctx:text(sr.x + 18, sr.y + 14, pet_name, 22, { 1.0, 0.92, 0.82, 0.95 }, "left")
    local status = (mode == "slot") and "napping" or (returning_home and "heading home" or (mode == "swipe" and "swiping!" or "roaming!"))
    ctx:text(sr.x + sr.w - 18, sr.y + 16, status, 18, { 0.65, 0.85, 1.0, 0.85 }, "right")

    if mode == "slot" then
      draw_cat(ctx, home_x, home_y + 4, 1.05, fur, 1, "sleep", false)
    elseif mode == "swipe" then
      draw_cat(ctx, home_x, home_y - 2, 1.0, fur, 1, "sit", startle_timer > 0)
    end
    ctx:clear_clip()
  elseif stage == "world_overlay" then
    if mode == "walk" or mode == "jump" or mode == "sit" then
      draw_cat(ctx, pos_x, pos_y, 1.0, fur, facing, mode, startle_timer > 0)
    elseif mode == "swipe" and sr then
      local reach = math.sin((paw_timer / 0.52) * math.pi)
      local px = home_x + (swipe_target_x - home_x) * reach * 0.75
      local py = (sr.y + sr.h) + (swipe_target_y - (sr.y + sr.h)) * reach
      ctx:line(home_x, sr.y + sr.h - 10, px, py, 16, fur)
      ctx:circle(px, py, 14, { 1.0, 0.95, 0.88, 0.96 })
      ctx:circle(px, py + 3, 5.5, { 1.0, 0.62, 0.70, 0.95 })
      ctx:circle(px - 6, py - 4, 2.8, { 1.0, 0.62, 0.70, 0.95 })
      ctx:circle(px, py - 6, 2.8, { 1.0, 0.62, 0.70, 0.95 })
      ctx:circle(px + 6, py - 4, 2.8, { 1.0, 0.62, 0.70, 0.95 })
    end
  end
end
