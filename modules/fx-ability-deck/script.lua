-- StreamDeck-Style Live Ability Deck!
-- Renders rich per-key LCD tiles with vector emblems, sweeping cooldown curtains, countdown timers,
-- charge counters + pip dots, ready-flash pulses, and instant SHIFT / CTRL modifier layer swapping!
-- Works both standalone (local interactive casting) and linked to `host/ability_sim.py` (or any game/app).

local layout = nil
local shift_held = false
local ctrl_held = false
local anim_t = 0.0

-- ANSI modifier key indices
local KEY_LSHIFT, KEY_RSHIFT = 60, 71
local KEY_LCTRL, KEY_RCTRL = 74, 81

-- Three rich StreamDeck-style key banks switched live by holding SHIFT or CTRL!
-- Key indices: 1=17, 2=18, 3=19, 4=20, 5=21, 6=22, Q=32, E=34, R=35, F=50
local decks = {
  base = {
    [17] = { name = "FIRE",   icon = "flame",     color = "#ff5c33", cd = 0.0, max_cd = 4.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [18] = { name = "FROST",  icon = "snow",      color = "#45d9ff", cd = 2.4, max_cd = 6.0,  charges = 2, max_charges = 2, ready_flash = 0 },
    [19] = { name = "BLINK",  icon = "bolt",      color = "#a86bff", cd = 0.0, max_cd = 8.0,  charges = 2, max_charges = 3, ready_flash = 0 },
    [20] = { name = "SHIELD", icon = "shield",    color = "#4dff9e", cd = 5.1, max_cd = 10.0, charges = 0, max_charges = 0, ready_flash = 0 },
    [21] = { name = "METEOR", icon = "star",      color = "#ff9f2e", cd = 9.5, max_cd = 14.0, charges = 0, max_charges = 0, ready_flash = 0 },
    [22] = { name = "POTION", icon = "heart",     color = "#ff4f79", cd = 0.0, max_cd = 12.0, charges = 4, max_charges = 5, ready_flash = 0 },
    [32] = { name = "SLASH",  icon = "sword",     color = "#f2c94c", cd = 0.0, max_cd = 2.5,  charges = 0, max_charges = 0, ready_flash = 0 },
    [34] = { name = "DASH",   icon = "arrow",     color = "#3ce8c6", cd = 1.6, max_cd = 5.0,  charges = 1, max_charges = 2, ready_flash = 0 },
    [35] = { name = "ULT",    icon = "crown",     color = "#ffd166", cd = 0.0, max_cd = 20.0, charges = 0, max_charges = 0, ult_pct = 92, ready_flash = 0 },
    [50] = { name = "PARRY",  icon = "ring",      color = "#68b5ff", cd = 0.0, max_cd = 3.5,  charges = 0, max_charges = 0, ready_flash = 0 },
  },
  shift = {
    [17] = { name = "MIC",    icon = "mic",       color = "#ff5964", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = true,  ready_flash = 0 },
    [18] = { name = "DEAFEN", icon = "ring",      color = "#ff8c42", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = false, ready_flash = 0 },
    [19] = { name = "CLIP",   icon = "cam",       color = "#4de1ff", cd = 0.0, max_cd = 5.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [20] = { name = "SCENE1", icon = "cam",       color = "#7c6bff", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = true,  ready_flash = 0 },
    [21] = { name = "SCENE2", icon = "cam",       color = "#4dffb8", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = false, ready_flash = 0 },
    [22] = { name = "LIVE",   icon = "star",      color = "#ff3b5c", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0, toggled = true,  ready_flash = 0 },
    [32] = { name = "PING",   icon = "bolt",      color = "#ffd166", cd = 0.0, max_cd = 3.0,  charges = 3, max_charges = 3, ready_flash = 0 },
    [34] = { name = "MOUNT",  icon = "arrow",     color = "#68d8ff", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [35] = { name = "RECALL", icon = "ring",      color = "#c084fc", cd = 0.0, max_cd = 8.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [50] = { name = "LOOT",   icon = "crown",     color = "#58f29c", cd = 0.0, max_cd = 1.5,  charges = 0, max_charges = 0, ready_flash = 0 },
  },
  ctrl = {
    [17] = { name = "BUILD",  icon = "bolt",      color = "#4de1ff", cd = 0.0, max_cd = 4.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [18] = { name = "TEST",   icon = "shield",    color = "#58f29c", cd = 0.0, max_cd = 5.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [19] = { name = "SYNC",   icon = "arrow",     color = "#a86bff", cd = 0.0, max_cd = 3.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [20] = { name = "DEPLOY", icon = "flame",     color = "#ff9f2e", cd = 0.0, max_cd = 12.0, charges = 0, max_charges = 0, ready_flash = 0 },
    [21] = { name = "LOGS",   icon = "ring",      color = "#68b5ff", cd = 0.0, max_cd = 1.5,  charges = 0, max_charges = 0, ready_flash = 0 },
    [22] = { name = "KILL",   icon = "star",      color = "#ff4f64", cd = 0.0, max_cd = 3.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [32] = { name = "GIT",    icon = "arrow",     color = "#ff7849", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [34] = { name = "DOCKER", icon = "shield",    color = "#38bdf8", cd = 0.0, max_cd = 4.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [35] = { name = "BENCH",  icon = "crown",     color = "#ffd166", cd = 0.0, max_cd = 6.0,  charges = 0, max_charges = 0, ready_flash = 0 },
    [50] = { name = "PRUSA",  icon = "cam",       color = "#fa6831", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0, ready_flash = 0 },
  },
}

function on_init()
  layout = keyboard.layout()
end

local function active_deck_name()
  if ctrl_held then return "ctrl" end
  if shift_held then return "shift" end
  return "base"
end

local function parse_hex(hex)
  if type(hex) ~= "string" then return { 0.4, 0.85, 1.0, 1.0 } end
  local s = hex:gsub("^#", "")
  if #s == 6 then
    local r = tonumber(s:sub(1, 2), 16) or 100
    local g = tonumber(s:sub(3, 4), 16) or 200
    local b = tonumber(s:sub(5, 6), 16) or 255
    return { r / 255.0, g / 255.0, b / 255.0, 1.0 }
  end
  return { 0.4, 0.85, 1.0, 1.0 }
end

function on_event(topic, payload)
  if topic == "input/keys/event" and type(payload) == "table" then
    local ki = payload.key_index
    local down = (payload.kind == "press")
    if ki == KEY_LSHIFT or ki == KEY_RSHIFT then
      shift_held = down
      return
    elseif ki == KEY_LCTRL or ki == KEY_RCTRL then
      ctrl_held = down
      return
    end

    if down then
      local dname = active_deck_name()
      local deck = decks[dname]
      local ab = deck and deck[ki]
      if ab then
        -- Interactive casting right on the keyboard!
        if ab.toggled ~= nil then
          ab.toggled = not ab.toggled
          ab.ready_flash = 0.65
        elseif ab.ult_pct ~= nil then
          if ab.ult_pct >= 100 then
            ab.ult_pct = 0
            ab.cd = ab.max_cd
            ab.ready_flash = 1.0
            bus.publish("fx/surface/impulse", { x = payload.x_px, y = payload.y_px, radius = 95, force = 1.5, kind = "shockwave", source = "module:fx-ability-deck" }, false)
          end
        elseif ab.max_charges > 0 then
          if ab.charges > 0 then
            ab.charges = ab.charges - 1
            if ab.cd <= 0 then ab.cd = ab.max_cd end
            ab.ready_flash = 0.5
          end
        elseif ab.cd <= 0 then
          ab.cd = ab.max_cd
          ab.ready_flash = 0.55
        end
        bus.publish("modules/ability-deck/cast", { deck = dname, key_index = ki, ability = ab.name }, false)
      end
    end
  elseif topic == "modules/ability-deck/state" and type(payload) == "table" then
    -- Accept live updates pushed from host/ability_sim.py or any external game/app!
    if type(payload.decks) == "table" then
      for dname, dmap in pairs(payload.decks) do
        if decks[dname] and type(dmap) == "table" then
          for kstr, info in pairs(dmap) do
            local ki = tonumber(kstr)
            if ki and decks[dname][ki] and type(info) == "table" then
              for fk, fv in pairs(info) do
                decks[dname][ki][fk] = fv
              end
            end
          end
        end
      end
    end
  end
end

function on_update(dt)
  anim_t = anim_t + dt
  for _, deck in pairs(decks) do
    for _, ab in pairs(deck) do
      if ab.ready_flash and ab.ready_flash > 0 then
        ab.ready_flash = math.max(0, ab.ready_flash - dt * 2.2)
      end
      if ab.cd and ab.cd > 0 then
        ab.cd = math.max(0, ab.cd - dt)
        if ab.cd == 0 then
          if ab.max_charges > 0 and ab.charges < ab.max_charges then
            ab.charges = ab.charges + 1
            if ab.charges < ab.max_charges then
              ab.cd = ab.max_cd
            end
          end
          ab.ready_flash = 0.9
        end
      end
      if ab.ult_pct ~= nil and ab.ult_pct < 100 and (ab.cd or 0) <= 0 then
        ab.ult_pct = math.min(100, ab.ult_pct + dt * 4.5)
        if ab.ult_pct >= 100 then ab.ready_flash = 1.0 end
      end
    end
  end
end

local function draw_icon(ctx, kind, cx, cy, col)
  local bright = { math.min(1, col[1] * 1.25), math.min(1, col[2] * 1.25), math.min(1, col[3] * 1.25), col[4] }
  if kind == "flame" then
    ctx:ellipse(cx, cy + 4, 14, 17, col)
    ctx:ellipse(cx, cy + 7, 7, 10, { 1.0, 0.92, 0.55, col[4] })
    ctx:line(cx, cy - 16, cx - 8, cy + 2, 6, col)
    ctx:line(cx, cy - 16, cx + 8, cy + 2, 6, col)
  elseif kind == "snow" then
    ctx:line(cx, cy - 16, cx, cy + 16, 3.2, bright)
    ctx:line(cx - 14, cy - 8, cx + 14, cy + 8, 3.2, bright)
    ctx:line(cx - 14, cy + 8, cx + 14, cy - 8, 3.2, bright)
    ctx:circle(cx, cy, 4.5, { 1, 1, 1, col[4] })
  elseif kind == "bolt" then
    ctx:line(cx + 4, cy - 17, cx - 7, cy + 1, 5.0, bright)
    ctx:line(cx - 7, cy + 1, cx + 5, cy + 1, 5.0, bright)
    ctx:line(cx + 5, cy + 1, cx - 5, cy + 18, 5.0, bright)
  elseif kind == "shield" then
    ctx:round_rect(cx - 13, cy - 15, 26, 20, 6, col, 2.0, bright)
    ctx:line(cx - 12, cy + 3, cx, cy + 17, 6.0, col)
    ctx:line(cx + 12, cy + 3, cx, cy + 17, 6.0, col)
    ctx:line(cx, cy - 10, cx, cy + 11, 2.5, { 1, 1, 1, col[4] * 0.8 })
  elseif kind == "heart" then
    ctx:circle(cx - 7, cy - 4, 9, col)
    ctx:circle(cx + 7, cy - 4, 9, col)
    ctx:line(cx - 13, cy + 1, cx, cy + 16, 8, col)
    ctx:line(cx + 13, cy + 1, cx, cy + 16, 8, col)
  elseif kind == "sword" then
    ctx:line(cx - 13, cy + 13, cx + 14, cy - 14, 5.0, bright)
    ctx:line(cx - 13, cy + 3, cx - 3, cy + 13, 4.0, col)
  elseif kind == "arrow" then
    ctx:line(cx - 14, cy + 4, cx + 13, cy - 6, 4.5, bright)
    ctx:line(cx + 13, cy - 6, cx + 2, cy - 13, 4.0, bright)
    ctx:line(cx + 13, cy - 6, cx + 6, cy + 6, 4.0, bright)
  elseif kind == "crown" then
    ctx:round_rect(cx - 15, cy + 4, 30, 9, 3, col, 0, { 0, 0, 0, 0 })
    ctx:line(cx - 14, cy + 5, cx - 14, cy - 9, 4.5, bright)
    ctx:line(cx, cy + 5, cx, cy - 14, 5.0, bright)
    ctx:line(cx + 14, cy + 5, cx + 14, cy - 9, 4.5, bright)
  elseif kind == "mic" then
    ctx:round_rect(cx - 6, cy - 15, 12, 18, 6, bright, 0, { 0, 0, 0, 0 })
    ctx:arc(cx, cy - 2, 11, 0, 180, 3.0, col)
    ctx:line(cx, cy + 9, cx, cy + 16, 3.0, col)
  elseif kind == "cam" then
    ctx:round_rect(cx - 15, cy - 10, 30, 20, 5, col, 2.0, bright)
    ctx:circle(cx, cy, 6.5, { 0.06, 0.08, 0.14, col[4] }, 2.2, { 1, 1, 1, col[4] })
  elseif kind == "star" then
    ctx:circle(cx, cy, 11, col)
    ctx:line(cx, cy - 17, cx, cy + 17, 3.5, bright)
    ctx:line(cx - 17, cy, cx + 17, cy, 3.5, bright)
  else
    ctx:circle(cx, cy, 12, { col[1], col[2], col[3], col[4] * 0.25 }, 3.8, bright)
  end
end

function on_render(ctx, stage)
  if stage ~= "world_overlay" or not layout or not layout.keys then return end
  local opacity = tonumber(module.config().opacity) or 0.88
  local dname = active_deck_name()
  local deck = decks[dname]
  if not deck then return end

  -- Subtle active-layer badge on Left Shift / Left Ctrl so you see which StreamDeck bank is active!
  local mod_key = (dname == "ctrl") and layout.keys[KEY_LCTRL + 1]
    or ((dname == "shift") and layout.keys[KEY_LSHIFT + 1] or nil)
  if mod_key then
    local mr = mod_key.rect
    local mc = (dname == "ctrl") and { 1.0, 0.62, 0.18, 0.98 } or { 0.30, 0.88, 1.0, 0.98 }
    ctx:round_rect(mr.x + 2, mr.y + 2, mr.w - 4, mr.h - 4, 10, { 0.04, 0.07, 0.12, 0.94 }, 2.5, mc)
    ctx:text(mod_key.center_x, mod_key.center_y - 11, string.upper(dname) .. " BANK", 18, mc, "center")
  end

  for ki, ab in pairs(deck) do
    local key = layout.keys[ki + 1]
    if key then
      local r = key.rect
      local c = parse_hex(ab.color)
      local on_cd = (ab.cd or 0) > 0 and (ab.max_charges == 0 or ab.charges == 0)
      local ult_ready = (ab.ult_pct ~= nil and ab.ult_pct >= 100)
      local pulse = ult_ready and (0.65 + 0.35 * math.sin(anim_t * 7.5)) or 0.0
      local flash = ab.ready_flash or 0.0

      ctx:clip(r.x, r.y, r.w, r.h)

      -- StreamDeck dark glass keycap tile with glowing border
      local bg_a = on_cd and (opacity * 0.88) or opacity
      local border_col = {
        math.min(1, c[1] + flash * 0.5 + pulse * 0.35),
        math.min(1, c[2] + flash * 0.5 + pulse * 0.35),
        math.min(1, c[3] + flash * 0.5 + pulse * 0.35),
        on_cd and 0.45 or (0.85 + pulse * 0.15),
      }
      local border_w = (ult_ready or ab.toggled == true) and 3.0 or 1.8
      ctx:round_rect(
        r.x + 2,
        r.y + 2,
        r.w - 4,
        r.h - 4,
        10,
        { 0.04 + c[1] * 0.10, 0.06 + c[2] * 0.10, 0.10 + c[3] * 0.10, bg_a },
        border_w,
        border_col
      )

      -- Icon emblem
      local icon_alpha = on_cd and 0.34 or 0.96
      draw_icon(ctx, ab.icon, key.center_x, key.center_y - 10, { c[1], c[2], c[3], icon_alpha })

      -- Sweeping vertical cooldown curtain + big crisp countdown timer
      if (ab.cd or 0) > 0 and (ab.max_cd or 1) > 0 then
        local frac = math.max(0.0, math.min(1.0, ab.cd / ab.max_cd))
        if on_cd then
          local ch = (r.h - 8) * frac
          ctx:rect(r.x + 4, r.y + r.h - 4 - ch, r.w - 8, ch, { 0.01, 0.02, 0.05, 0.68 })
          local cd_txt = (ab.cd >= 10) and string.format("%d", math.ceil(ab.cd)) or string.format("%.1f", ab.cd)
          ctx:text(key.center_x, key.center_y - 18, cd_txt, 28, { 1.0, 1.0, 1.0, 0.98 }, "center")
        else
          -- Subtle recharge bar at top when still has remaining charges!
          ctx:rect(r.x + 8, r.y + 6, (r.w - 16) * (1.0 - frac), 4, { c[1], c[2], c[3], 0.9 })
        end
      end

      -- Ultimate charge progress ring & % badge
      if ab.ult_pct ~= nil then
        local pct = math.floor(ab.ult_pct + 0.5)
        ctx:arc(key.center_x, key.center_y - 10, 22, -90, -90 + (ab.ult_pct / 100.0) * 360.0, 3.2, border_col)
        if pct < 100 then
          ctx:text(r.x + r.w - 8, r.y + 6, string.format("%d%%", pct), 13, { 1.0, 0.94, 0.65, 0.95 }, "right")
        end
      end

      -- Charge count badge (top-right pill) + pip dots
      if (ab.max_charges or 0) > 0 then
        local bx = r.x + r.w - 24
        local by = r.y + 6
        ctx:round_rect(bx, by, 18, 18, 6, { 0.05, 0.07, 0.12, 0.92 }, 1.2, c)
        ctx:text(bx + 9, by + 1, tostring(ab.charges), 14, { 1, 1, 1, 0.98 }, "center")
      elseif ab.toggled ~= nil then
        -- StreamDeck active LED dot in top-right
        local led = ab.toggled and { 0.35, 1.0, 0.55, 1.0 } or { 0.4, 0.45, 0.52, 0.5 }
        ctx:circle(r.x + r.w - 14, r.y + 14, 4.5, led)
      end

      -- Crisp ability label at bottom of keycap
      ctx:text(key.center_x, r.y + r.h - 22, ab.name, 14, { 0.94, 0.96, 1.0, on_cd and 0.55 or 0.95 }, "center")

      ctx:clear_clip()
    end
  end
end
