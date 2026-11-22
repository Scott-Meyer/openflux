-- 3D Water Ripple (`fx-water-ripple-3d`)
-- Drives the fixed-60Hz 480x270 viscosity-damped water simulation bending the live video
-- underneath with warm sunlit rim glints.
-- Modes (per keydown, ignoring auto-repeat):
--   "ripple" -> crisp ring per key (radius 46 px, amplitude 0.9)
--   "gentle" -> broad soft swell per key (radius 140 px, amplitude 0.5)

local pending_impulses = {}
local held_keys = {}

function on_event(topic, payload)
  if topic == "input/keys/event" and type(payload) == "table" then
    local idx = payload.key_index
    if payload.kind == "release" then
      if idx ~= nil then held_keys[idx] = nil end
      return
    end
    if payload.kind == "press" then
      -- Ignore auto-repeat while key is already held down
      if idx ~= nil and held_keys[idx] then return end
      if idx ~= nil then held_keys[idx] = true end

      local mode = tostring(module.config().mode or "ripple")
      local radius = (mode == "gentle") and 140.0 or 46.0
      local amp = (mode == "gentle") and 0.5 or 0.9
      table.insert(pending_impulses, {
        x = payload.x_px,
        y = payload.y_px,
        radius = radius,
        force = amp,
      })
    end
  elseif topic == "fx/surface/impulse" and type(payload) == "table" then
    local mode = tostring(module.config().mode or "ripple")
    local def_r = (mode == "gentle") and 140.0 or 46.0
    local def_a = (mode == "gentle") and 0.5 or 0.9
    table.insert(pending_impulses, {
      x = payload.x or 960,
      y = payload.y or 540,
      radius = payload.radius or def_r,
      force = payload.force or def_a,
    })
  end
end

function on_render(ctx, stage)
  if stage ~= "distortion" then return end
  local cfg = module.config()
  local damping = tonumber(cfg.damping) or 0.988
  local refraction = tonumber(cfg.refraction) or 25.0
  local specular = tonumber(cfg.specular) or 1.0
  local tint = module.color("tint", "#ffdb9e")

  ctx:wave_params(damping, 0.5, refraction, specular, tint)
  for _, imp in ipairs(pending_impulses) do
    ctx:wave_impulse(imp.x, imp.y, imp.radius, imp.force)
  end
  pending_impulses = {}
end
