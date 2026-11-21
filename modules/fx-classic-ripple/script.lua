local ripples = {}

function on_event(topic, payload)
  if topic == "input/keys/event" and payload.kind == "press" then
    table.insert(ripples, { x = payload.x_px, y = payload.y_px, age = 0 })
  elseif topic == "fx/surface/impulse" then
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
