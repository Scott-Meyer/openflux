-- 3D Water Ripple: drives the compositor's GPU 2D wave heightfield simulation so
-- key presses AND shared `fx/surface/impulse` events (like the Idle Pet's paws or CLI shockwaves)
-- physically bend and shimmer whatever video or shader is playing underneath!

local pending_impulses = {}

function on_event(topic, payload)
  if topic == "input/keys/event" and payload.kind == "press" then
    local depth_scale = 0.75
    if payload.depth_um and payload.depth_um > 0 then
      depth_scale = 0.45 + math.min(payload.depth_um / 2800.0, 1.0) * 0.75
    end
    table.insert(pending_impulses, {
      x = payload.x_px,
      y = payload.y_px,
      radius = 52.0,
      force = 1.15 * depth_scale,
    })
  elseif topic == "fx/surface/impulse" then
    table.insert(pending_impulses, {
      x = payload.x or 960,
      y = payload.y or 540,
      radius = payload.radius or 58.0,
      force = (payload.force or 0.9) * 1.15,
    })
  end
end

function on_render(ctx, stage)
  if stage ~= "distortion" then return end
  local cfg = module.config()
  local damping = tonumber(cfg.damping) or 0.986
  local speed = tonumber(cfg.speed) or 0.44
  local refraction = tonumber(cfg.refraction) or 46.0
  local specular = tonumber(cfg.specular) or 0.72
  local tint = module.color("tint", "#9ce8ff")

  ctx:wave_params(damping, speed, refraction, specular, tint)
  for _, imp in ipairs(pending_impulses) do
    ctx:wave_impulse(imp.x, imp.y, imp.radius, imp.force)
  end
  pending_impulses = {}
end
