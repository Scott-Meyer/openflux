local ripples = {}
local held = {}
local MAX_RIPPLES = 16

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
      table.insert(ripples, { x = payload.x_px, y = payload.y_px, age = 0 })
    end
  elseif topic == "fx/surface/impulse" and type(payload) == "table" then
    if #ripples >= MAX_RIPPLES then table.remove(ripples, 1) end
    table.insert(ripples, { x = payload.x or 960, y = payload.y or 540, age = 0 })
  end
end

function on_update(dt)
  local kept = {}
  for _, r in ipairs(ripples) do
    r.age = r.age + dt
    if r.age < 0.85 then table.insert(kept, r) end
  end
  ripples = kept
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local c = module.color("color", "#68ffbe")
  for _, r in ipairs(ripples) do
    local t = r.age / 0.85
    local alpha = (1.0 - t) * 0.85
    ctx:key_glow("ripple", r.x, r.y, 2.0, 2.0, 24.0 + t * 340.0, 6.0, alpha, { c[1], c[2], c[3], alpha })
  end
end
