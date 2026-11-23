-- Classic Glowing Rings (`fx-classic-rings`) — clean-room v1 parity + soft luminous glow:
-- Crisp circular glowing ring inside each held keycap (bright core + soft outer bloom halo),
-- gentle +18px press pulse over 500ms, and smooth quadratic 560ms fade-out on release.

local keys = {} -- [key_index] = { x, y, unit, pressed, press_age, rel_age }

function on_event(topic, payload)
  if topic ~= "input/keys/event" or type(payload) ~= "table" then return end
  local idx = payload.key_index
  if idx == nil then return end
  local unit = math.min(tonumber(payload.w_px) or 105, tonumber(payload.h_px) or 105)
  if payload.kind == "press" then
    local existing = keys[idx]
    if existing and existing.pressed then return end
    keys[idx] = {
      x = payload.x_px,
      y = payload.y_px,
      unit = unit,
      pressed = true,
      press_age = 0.0,
      rel_age = 0.0,
    }
  elseif payload.kind == "release" then
    local existing = keys[idx]
    if existing then
      existing.pressed = false
      existing.rel_age = 0.0
    end
  end
end

function on_update(dt)
  for idx, k in pairs(keys) do
    k.press_age = k.press_age + dt
    if not k.pressed then
      k.rel_age = k.rel_age + dt
      if k.rel_age >= 0.56 then
        keys[idx] = nil
      end
    end
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local c = module.color("color", "#4df0ff")
  local cr = math.min(1.0, c[1] * 0.8 + 0.2)
  local cg = math.min(1.0, c[2] * 0.8 + 0.2)
  local cb = math.min(1.0, c[3] * 0.8 + 0.2)

  for _, k in pairs(keys) do
    local glow = 1.0
    if not k.pressed then
      local lin = math.max(0.0, 1.0 - k.rel_age / 0.56)
      glow = lin * lin
    end
    if glow > 0.01 then
      local base_r = k.unit * 0.34
      -- Soft wide outer bloom halo
      ctx:key_glow("ripple", k.x, k.y, 2.0, 2.0, base_r + 3.0, 11.0, glow * 0.42, { c[1], c[2], c[3], glow * 0.42 })
      -- Sharp bright inner core ring
      ctx:key_glow("ripple", k.x, k.y, 2.0, 2.0, base_r, 3.0, glow * 1.0, { cr, cg, cb, glow * 1.0 })
      -- Gentle expanding press pulse over 500ms
      if k.press_age < 0.50 then
        local pt = k.press_age / 0.50
        local pr = base_r + pt * 18.0
        local pa = (1.0 - pt) * 0.72
        ctx:key_glow("ripple", k.x, k.y, 2.0, 2.0, pr, 3.0, pa, { cr, cg, cb, pa })
      end
    end
  end
end
