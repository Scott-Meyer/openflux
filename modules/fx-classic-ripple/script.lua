-- Classic 2D Ripple (`fx-classic-ripple`) — clean-room v1 parity:
-- Single crisp circular ring per press expanding with smooth ease-out `1 - (1 - t)^2`
-- (~2.1 key-widths reach, thin 3.8px ring, graceful fade).

local ripples = {}
local held = {}
local MAX_RIPPLES = 16
local DURATION = 1.65

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
  for _, r in ipairs(ripples) do
    local pt = math.min(1.0, r.age / DURATION)
    local ease = 1.0 - (1.0 - pt) * (1.0 - pt)
    local pr = r.unit * 0.34 + ease * (r.unit * 2.1)
    local alpha = (1.0 - pt) * 0.92
    ctx:key_glow("ripple", r.x, r.y, 2.0, 2.0, pr, 3.8, alpha, { c[1], c[2], c[3], alpha })
  end
end
