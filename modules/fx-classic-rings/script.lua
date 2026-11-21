local rings = {}

function on_event(topic, payload)
  if topic == "input/keys/event" and payload.kind == "press" then
    table.insert(rings, { x = payload.x_px, y = payload.y_px, w = payload.w_px, h = payload.h_px, age = 0 })
  end
end

function on_update(dt)
  local kept = {}
  for _, r in ipairs(rings) do
    r.age = r.age + dt
    if r.age < 0.55 then table.insert(kept, r) end
  end
  rings = kept
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local c = module.color("color", "#4df0ff")
  for _, r in ipairs(rings) do
    local t = r.age / 0.55
    local alpha = (1.0 - t) * 0.95
    ctx:key_glow("ring", r.x, r.y, r.w, r.h, 8.0 + t * 88.0, 5.5, alpha, { c[1], c[2], c[3], alpha })
  end
end
