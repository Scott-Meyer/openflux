-- Classic 2D Ripple (`fx-classic-ripple`) — clean-room v1 parity + soft luminous glow:
-- Single crisp circular ring per press expanding gracefully with smooth ease-out `1 - (1 - t)^2`,
-- lifted bright core, and a subtle soft bloom halo around the ring.

local ripples = {}
local held = {}
local MAX_RIPPLES = 16
local DURATION = 2.05

function on_event(topic, payload)
  if topic == "input/keys/event" and type(payload) == "table" then
    local idx = payload.key_index
    if payload.kind == "release" then
      if idx ~= nil then held[idx] = nil end
      return
    end
    if payload.kind == "press" then
      if idx ~= nil and held[idx] then return end
      if idx ~= nil then held[idx] = true end
      if #ripples >= MAX_RIPPLES then table.remove(ripples, 1) end
      local unit = math.min(tonumber(payload.w_px) or 105, tonumber(payload.h_px) or 105)
      table.insert(ripples, { x = payload.x_px, y = payload.y_px, unit = unit, age = 0.0 })
    end
  elseif topic == "fx/surface/impulse" and type(payload) == "table" then
    if #ripples >= MAX_RIPPLES then table.remove(ripples, 1) end
    table.insert(ripples, { x = payload.x or 960, y = payload.y or 540, unit = 105.0, age = 0.0 })
  end
end

function on_update(dt)
  local kept = {}
  for _, r in ipairs(ripples) do
    r.age = r.age + dt
    if r.age < DURATION then table.insert(kept, r) end
  end
  ripples = kept
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local c = module.color("color", "#68ffbe")
  -- Slightly lifted luminous core tint
  local cr = math.min(1.0, c[1] * 0.78 + 0.22)
  local cg = math.min(1.0, c[2] * 0.78 + 0.22)
  local cb = math.min(1.0, c[3] * 0.78 + 0.22)

  for _, r in ipairs(ripples) do
    local pt = math.min(1.0, r.age / DURATION)
    local ease = 1.0 - (1.0 - pt) * (1.0 - pt)
    local pr = r.unit * 0.34 + ease * (r.unit * 2.15)
    local fade = 1.0 - pt
    local glow_a = (fade * fade) * 0.34
    local core_a = fade * 1.0

    -- Soft outer bloom halo around the ring
    ctx:key_glow("ripple", r.x, r.y, 2.0, 2.0, pr, 14.0, glow_a, { c[1], c[2], c[3], glow_a })
    -- Crisp bright inner core ring
    ctx:key_glow("ripple", r.x, r.y, 2.0, 2.0, pr, 4.0, core_a, { cr, cg, cb, core_a })
  end
end
