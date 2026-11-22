-- Mochi the Cat — OpenFlux v2 Architectural North Star Companion!
-- Mario-style 2D platformer cat that walks/runs smoothly along physical keycap rows,
-- pauses to sit/paw keys, leaps up/down between rows with parabolic jump arcs,
-- peeks & swipes down from her top-bar slot when you type on upper rows,
-- and emits `fx/surface/impulse` on footsteps & landings so 3D Water Ripple reacts under her paws!

local layout = nil
local rows = {} -- rows[0..5] = { y_top, min_x, max_x, keys = {...} }
local mode = "slot" -- "slot" | "swipe" | "walk" | "sit" | "jump" | "return_home"
local cur_row = 0
local pos_x, pos_y = 1650.0, 120.0
local home_x, home_y = 1650.0, 120.0
local facing = -1 -- -1 left, +1 right
local walk_speed = 185.0
local walk_timer = 0.0
local step_accum = 0.0
local sit_timer = 0.0
local idle_timer = 0.0
local paw_timer = 0.0
local swipe_target_x, swipe_target_y = 1650.0, 340.0
local startle_timer = 0.0
local anim_t = 0.0

-- Parabolic Mario jump state
local j_sx, j_sy = 0.0, 0.0
local j_tx, j_ty = 0.0, 0.0
local j_t, j_dur = 0.0, 0.42
local j_arc = 75.0
local j_dest_row = 0
local j_returning = false
local action_count = 0

local function build_row_platforms()
  rows = {}
  if not layout or not layout.keys then return end
  for _, k in ipairs(layout.keys) do
    local r = k.row or 0
    if not rows[r] then
      rows[r] = { y_top = k.rect.y, min_x = k.rect.x + 28, max_x = k.rect.x + k.rect.w - 28, keys = {} }
    end
    local rr = rows[r]
    if k.rect.y < rr.y_top then rr.y_top = k.rect.y end
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
    radius = radius or 52.0,
    force = force or 0.75,
    kind = "paw",
    source = "module:companion-idle-pet",
  }, false)
end

local function start_jump(tx, ty, dest_row, arc_h, returning)
  j_sx, j_sy = pos_x, pos_y
  j_tx, j_ty = tx, ty
  j_dest_row = dest_row
  j_arc = arc_h or 72.0
  j_t = 0.0
  local dist = math.sqrt((tx - pos_x) ^ 2 + (ty - pos_y) ^ 2)
  j_dur = math.max(0.28, math.min(0.55, 0.26 + dist / 950.0))
  j_returning = returning == true
  if tx > pos_x + 4 then facing = 1 elseif tx < pos_x - 4 then facing = -1 end
  mode = "jump"
end

local function enter_key_platform(row_idx, target_x)
  local rr = rows[row_idx] or rows[0]
  if not rr then return end
  local x = math.max(rr.min_x + 40, math.min(rr.max_x - 40, target_x or pos_x))
  -- Stand right on top of the physical key row!
  start_jump(x, rr.y_top + 14, row_idx, 78.0, false)
end

function on_event(topic, payload)
  if topic == "modules/companion-idle-pet/command" and type(payload) == "table" then
    if payload.action == "roam" then
      local tk = payload.target_key
      if tk and layout and layout.keys and layout.keys[tk + 1] then
        local k = layout.keys[tk + 1]
        enter_key_platform(k.row or 1, k.center_x)
      else
        enter_key_platform(0, home_x - 220)
      end
    elseif payload.action == "home" then
      start_jump(home_x, home_y, 0, 88.0, true)
    end
  elseif topic == "input/keys/event" and payload.kind == "press" then
    idle_timer = 0.0
    local dx = (payload.x_px or 0) - pos_x
    local dy = (payload.y_px or 0) - pos_y
    local dist = math.sqrt(dx * dx + dy * dy)

    if mode == "slot" and (payload.row or 9) <= 1 and math.abs((payload.x_px or 0) - home_x) < 460 then
      mode = "swipe"
      paw_timer = 0.52
      swipe_target_x = payload.x_px
      swipe_target_y = payload.y_px
    elseif (mode == "walk" or mode == "sit") and dist < 210 then
      -- Startled Mario hop away from the pressed key!
      startle_timer = 0.55
      local next_row = (cur_row < 4) and (cur_row + 1) or math.max(0, cur_row - 1)
      local hop_dir = (dx >= 0) and -1 or 1
      facing = hop_dir
      enter_key_platform(next_row, pos_x + hop_dir * 220)
    end
  elseif topic == "fx/surface/impulse" then
    if payload.source == "module:companion-idle-pet" then return end
    local dx = (payload.x or 0) - pos_x
    local dy = (payload.y or 0) - pos_y
    if math.sqrt(dx * dx + dy * dy) < 220 and (payload.force or 0) >= 0.75 then
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
    if idle_timer >= idle_limit then
      facing = -1
      enter_key_platform(0, home_x - 160)
    end
  elseif mode == "swipe" then
    paw_timer = paw_timer - dt
    if paw_timer <= 0 then mode = "slot" end
  elseif mode == "jump" then
    j_t = j_t + dt / j_dur
    if j_t >= 1.0 then
      pos_x, pos_y = j_tx, j_ty
      if j_returning then
        mode = "slot"
        idle_timer = 0.0
      else
        cur_row = j_dest_row
        -- Mario landing splash!
        emit_paw_impulse(pos_x, pos_y + 24, 0.95, 58.0)
        mode = "walk"
        walk_timer = 1.6 + ((action_count * 37) % 24) * 0.1
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
    pos_x = pos_x + facing * walk_speed * dt
    pos_y = rr.y_top + 14
    walk_timer = walk_timer - dt
    step_accum = step_accum + walk_speed * dt

    -- Soft footstep ripple every ~1.5 keys while trotting along the row
    if step_accum >= 155.0 then
      step_accum = 0.0
      emit_paw_impulse(pos_x, pos_y + 26, 0.35, 36.0)
    end

    -- Turn around cleanly at keyboard edges (or jump down a row!)
    if pos_x <= rr.min_x + 30 then
      pos_x = rr.min_x + 30
      facing = 1
      if cur_row < 5 and (action_count % 2 == 0) then
        enter_key_platform(cur_row + 1, pos_x + 90)
        return
      end
    elseif pos_x >= rr.max_x - 30 then
      pos_x = rr.max_x - 30
      facing = -1
      if cur_row < 5 and (action_count % 2 == 1) then
        enter_key_platform(cur_row + 1, pos_x - 90)
        return
      end
    end

    if walk_timer <= 0 then
      action_count = action_count + 1
      local choice = action_count % 5
      if choice == 0 and action_count >= 10 then
        -- Hop back up to home slot for a quick nap
        start_jump(home_x, home_y, 0, 95.0, true)
      elseif choice == 1 or choice == 3 then
        -- Mario-style shift DOWN a row (or hop back up from bottom row!)
        local next_row = (cur_row < 5) and (cur_row + 1) or math.max(0, cur_row - 2)
        local shift_x = pos_x + facing * 130
        enter_key_platform(next_row, shift_x)
      elseif choice == 2 then
        -- Mario-style hop UP a row!
        local next_row = (cur_row > 0) and (cur_row - 1) or 1
        enter_key_platform(next_row, pos_x + facing * 110)
      else
        -- Pause to sit on a key and bat at it
        mode = "sit"
        sit_timer = 1.15
        emit_paw_impulse(pos_x, pos_y + 24, 0.72, 48.0)
      end
    end
  elseif mode == "sit" then
    sit_timer = sit_timer - dt
    if sit_timer <= 0 then
      if (action_count % 3) == 0 then facing = -facing end
      mode = "walk"
      walk_timer = 1.8 + ((action_count * 23) % 18) * 0.1
    end
  end
end

-- Helper to draw a pointed triangular cat ear using stacked tapered strokes
local function draw_cat_ear(ctx, bx, by, tip_x, tip_y, base_w, fur, inner)
  ctx:line(bx - base_w * 0.45, by, tip_x, tip_y, base_w * 0.48, fur)
  ctx:line(bx + base_w * 0.45, by, tip_x, tip_y, base_w * 0.48, fur)
  ctx:line(bx, by, tip_x, tip_y, base_w * 0.56, fur)
  ctx:line(bx, by - 1, bx + (tip_x - bx) * 0.68, by + (tip_y - by) * 0.68, base_w * 0.28, inner)
end

-- Expressive side-profile platformer cat (walking/jumping) or cozy front/sitting cat!
local function draw_cat(ctx, x, y, sc, fur, dir, pose, startled)
  local stripe = { fur[1] * 0.68, fur[2] * 0.62, fur[3] * 0.58, 0.95 }
  local cream = { 1.0, 0.96, 0.90, 0.98 }
  local pink = { 1.0, 0.62, 0.70, 0.96 }
  local dark = { 0.08, 0.10, 0.13, 1.0 }
  local eye_col = { 0.42, 0.95, 0.62, 1.0 }
  local shadow = { 0.0, 0.0, 0.0, 0.36 }

  if pose == "sleep" then
    -- Curled sleeping cat loaf in top-bar bed
    local breathe = math.sin(anim_t * 2.2) * 1.6 * sc
    ctx:ellipse(x, y + 16 * sc, 38 * sc, 10 * sc, shadow)
    -- Curled tail wrapped around front
    ctx:arc(x + 2 * sc, y + 6 * sc, 26 * sc, 10, 155, 8 * sc, fur)
    -- Plump sleeping body + tabby stripes
    ctx:ellipse(x - 4 * sc, y + 2 * sc - breathe * 0.4, 30 * sc, 19 * sc + breathe * 0.4, fur)
    ctx:line(x - 14 * sc, y - 12 * sc, x - 12 * sc, y - 2 * sc, 4 * sc, stripe)
    ctx:line(x - 4 * sc, y - 14 * sc, x - 2 * sc, y - 3 * sc, 4 * sc, stripe)
    -- Tucked head resting on paws
    local hx = x + 16 * sc
    local hy = y - 2 * sc
    draw_cat_ear(ctx, hx - 10 * sc, hy - 12 * sc, hx - 15 * sc, hy - 26 * sc, 11 * sc, fur, pink)
    draw_cat_ear(ctx, hx + 8 * sc, hy - 12 * sc, hx + 13 * sc, hy - 26 * sc, 11 * sc, fur, pink)
    ctx:ellipse(hx, hy, 17 * sc, 14 * sc, fur)
    ctx:ellipse(hx, hy + 4 * sc, 9 * sc, 6 * sc, cream)
    -- Closed happy sleeping eyes
    ctx:arc(hx - 6 * sc, hy - 1 * sc, 4 * sc, 15, 165, 2.2 * sc, dark)
    ctx:arc(hx + 6 * sc, hy - 1 * sc, 4 * sc, 15, 165, 2.2 * sc, dark)
    ctx:circle(hx, hy + 3 * sc, 2.2 * sc, pink)
    -- Little floating "z"
    local zy = (anim_t * 14) % 18
    ctx:text(hx + 22 * sc, hy - 18 * sc - zy, "z", 14 * sc, { 0.85, 0.92, 1.0, 0.75 }, "center")
    return
  end

  if pose == "walk" or pose == "jump" then
    local stride = (pose == "walk") and math.sin(anim_t * 15.0) or 0.9
    local bob = (pose == "walk") and math.abs(math.sin(anim_t * 15.0)) * -3.5 * sc or 0.0
    local cy = y + bob

    if pose == "walk" then
      ctx:ellipse(x, y + 25 * sc, 32 * sc, 7 * sc, shadow)
    end

    -- Swishing S-curve cat tail behind the body
    local wag = math.sin(anim_t * 8.0) * 10 * sc
    local tx0 = x - dir * 24 * sc
    local ty0 = cy + 2 * sc
    local tx1 = x - dir * 38 * sc
    local ty1 = cy - 12 * sc + wag * 0.5
    local tx2 = x - dir * 44 * sc
    local ty2 = cy - 26 * sc + wag
    ctx:line(tx0, ty0, tx1, ty1, 7.5 * sc, fur)
    ctx:line(tx1, ty1, tx2, ty2, 6.5 * sc, fur)
    ctx:circle(tx2, ty2, 3.6 * sc, cream)

    -- Back legs
    local leg_a = stride * 10 * sc
    local leg_b = -stride * 10 * sc
    ctx:line(x - dir * 15 * sc, cy + 10 * sc, x - dir * 18 * sc + leg_b, cy + 23 * sc, 7 * sc, stripe)
    ctx:circle(x - dir * 18 * sc + leg_b, cy + 24 * sc, 4.2 * sc, cream)
    ctx:line(x + dir * 14 * sc, cy + 10 * sc, x + dir * 16 * sc + leg_a, cy + 23 * sc, 7 * sc, stripe)
    ctx:circle(x + dir * 16 * sc + leg_a, cy + 24 * sc, 4.2 * sc, cream)

    -- Sleek elongated cat torso + tabby stripes + cream chest
    ctx:ellipse(x, cy + 5 * sc, 27 * sc, 14 * sc, fur)
    ctx:ellipse(x + dir * 13 * sc, cy + 8 * sc, 11 * sc, 9 * sc, cream)
    ctx:line(x - dir * 8 * sc, cy - 8 * sc, x - dir * 6 * sc, cy + 2 * sc, 3.8 * sc, stripe)
    ctx:line(x + dir * 1 * sc, cy - 8 * sc, x + dir * 2 * sc, cy + 2 * sc, 3.8 * sc, stripe)

    -- Front legs
    ctx:line(x - dir * 11 * sc, cy + 11 * sc, x - dir * 10 * sc + leg_a, cy + 24 * sc, 7.5 * sc, fur)
    ctx:circle(x - dir * 10 * sc + leg_a, cy + 25 * sc, 4.5 * sc, cream)
    ctx:line(x + dir * 17 * sc, cy + 10 * sc, x + dir * 18 * sc + leg_b, cy + 24 * sc, 7.5 * sc, fur)
    ctx:circle(x + dir * 18 * sc + leg_b, cy + 25 * sc, 4.5 * sc, cream)

    -- Head turned forward in travel direction with sharp pointed cat ears & whiskers
    local hx = x + dir * 23 * sc
    local hy = cy - 11 * sc
    local ear_lift = startled and 6 * sc or 0
    draw_cat_ear(ctx, hx - dir * 8 * sc, hy - 10 * sc, hx - dir * 13 * sc, hy - 27 * sc - ear_lift, 11 * sc, fur, pink)
    draw_cat_ear(ctx, hx + dir * 6 * sc, hy - 10 * sc, hx + dir * 10 * sc, hy - 26 * sc - ear_lift, 11 * sc, fur, pink)
    ctx:ellipse(hx, hy, 16.5 * sc, 14.5 * sc, fur)
    ctx:ellipse(hx + dir * 6 * sc, hy + 4 * sc, 8 * sc, 6 * sc, cream)

    -- Expressive green cat eye + vertical slit pupil
    local ex = hx + dir * 7 * sc
    local ey = hy - 2 * sc
    ctx:ellipse(ex, ey, 4.4 * sc, 4.8 * sc, eye_col)
    ctx:ellipse(ex + dir * 1.2 * sc, ey, startled and 2.6 * sc or 1.6 * sc, 3.8 * sc, dark)
    ctx:circle(ex + dir * 1.5 * sc, ey - 1.5 * sc, 1.2 * sc, { 1, 1, 1, 1 })

    -- Pink nose & crisp cat whiskers
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

  -- Front-sitting upright cat pose (for "sit" and alert slot pose)
  local cy = y + math.sin(anim_t * 4.0) * 1.5 * sc
  ctx:ellipse(x, y + 26 * sc, 28 * sc, 8 * sc, shadow)
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
  -- Forehead tabby "M" stripes
  ctx:line(x - 4 * sc, hy - 13 * sc, x - 3 * sc, hy - 7 * sc, 2.2 * sc, stripe)
  ctx:line(x, hy - 14 * sc, x, hy - 7 * sc, 2.2 * sc, stripe)
  ctx:line(x + 4 * sc, hy - 13 * sc, x + 3 * sc, hy - 7 * sc, 2.2 * sc, stripe)
  -- Cat eyes
  ctx:ellipse(x - 7 * sc, hy - 1 * sc, 4.2 * sc, 4.6 * sc, eye_col)
  ctx:ellipse(x + 7 * sc, hy - 1 * sc, 4.2 * sc, 4.6 * sc, eye_col)
  ctx:ellipse(x - 7 * sc, hy - 1 * sc, 1.7 * sc, 3.6 * sc, dark)
  ctx:ellipse(x + 7 * sc, hy - 1 * sc, 1.7 * sc, 3.6 * sc, dark)
  ctx:circle(x - 5.5 * sc, hy - 2.5 * sc, 1.2 * sc, { 1, 1, 1, 1 })
  ctx:circle(x + 8.5 * sc, hy - 2.5 * sc, 1.2 * sc, { 1, 1, 1, 1 })
  -- Pink nose + whiskers
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
    local status = (mode == "slot") and "napping" or (mode == "swipe" and "swiping!" or "roaming!")
    ctx:text(sr.x + sr.w - 18, sr.y + 16, status, 18, { 0.65, 0.85, 1.0, 0.85 }, "right")

    if mode == "slot" then
      draw_cat(ctx, home_x, home_y + 4, 1.05, fur, 1, "sleep", false)
    elseif mode == "swipe" then
      draw_cat(ctx, home_x, home_y - 2, 1.0, fur, 1, "sit", startle_timer > 0)
    end
    ctx:clear_clip()
  elseif stage == "world_overlay" then
    if mode == "walk" or mode == "jump" or mode == "sit" then
      draw_cat(ctx, pos_x, pos_y, 1.05, fur, facing, mode, startle_timer > 0)
    elseif mode == "swipe" and sr then
      local reach = math.sin((paw_timer / 0.52) * math.pi)
      local px = home_x + (swipe_target_x - home_x) * reach * 0.75
      local py = (sr.y + sr.h) + (swipe_target_y - (sr.y + sr.h)) * reach
      ctx:line(home_x, sr.y + sr.h - 10, px, py, 16, fur)
      ctx:circle(px, py, 14, { 1.0, 0.95, 0.88, 0.96 })
      -- Cute pink toe beans!
      ctx:circle(px, py + 3, 5.5, { 1.0, 0.62, 0.70, 0.95 })
      ctx:circle(px - 6, py - 4, 2.8, { 1.0, 0.62, 0.70, 0.95 })
      ctx:circle(px, py - 6, 2.8, { 1.0, 0.62, 0.70, 0.95 })
      ctx:circle(px + 6, py - 4, 2.8, { 1.0, 0.62, 0.70, 0.95 })
    end
  end
end
