-- Analog Depth + Velocity Glow ("More on the way down, less on the way up")
-- Uses continuous Hall-effect travel depth (0..2800 um) and signed press velocity.

local layout = nil
local active = {} -- key_index -> { x, y, w, h, depth, vel, smooth_spread, smooth_intensity, age }

function on_init()
  layout = keyboard.layout()
end

local function key_dims(key_index)
  if layout and layout.keys then
    local k = layout.keys[key_index + 1]
    if k then return k.rect.w, k.rect.h, k.center_x, k.center_y end
  end
  return 105, 105, 960, 540
end

function on_event(topic, payload)
  if topic == "input/keys/analog" and type(payload.samples) == "table" then
    for _, s in ipairs(payload.samples) do
      local w, h, cx, cy = key_dims(s.key_index)
      local st = active[s.key_index] or {
        x = s.x_px or cx,
        y = s.y_px or cy,
        w = w,
        h = h,
        depth = 0,
        vel = 0,
        smooth_spread = 0,
        smooth_intensity = 0,
        age = 0,
      }
      st.x = s.x_px or cx
      st.y = s.y_px or cy
      st.depth = s.depth_norm or 0
      st.vel = s.velocity_norm or 0
      st.age = 0
      active[s.key_index] = st
    end
  elseif topic == "input/keys/event" then
    local w, h, cx, cy = key_dims(payload.key_index)
    local st = active[payload.key_index] or {
      x = payload.x_px or cx,
      y = payload.y_px or cy,
      w = payload.w_px or w,
      h = payload.h_px or h,
      depth = 0,
      vel = 0,
      smooth_spread = 0,
      smooth_intensity = 0,
      age = 0,
    }
    if payload.kind == "press" then
      st.depth = math.max(st.depth, (payload.depth_um or 2100) / 2800.0)
      st.vel = 0.85
    else
      st.vel = -0.75
    end
    st.age = 0
    active[payload.key_index] = st
  end
end

function on_update(dt)
  local cfg = module.config()
  local max_spread = tonumber(cfg.max_spread_px) or 145.0

  for k, st in pairs(active) do
    st.age = st.age + dt
    -- If no fresh analog frame arrived, gently decay depth/velocity.
    if st.age > 0.12 then
      st.depth = math.max(0, st.depth - dt * 1.9)
      st.vel = -0.55
    end

    -- Asymmetric response: pushing DOWN spreads much wider & brighter across neighbors;
    -- lifting UP contracts tighter & softer around the key!
    local dir_boost = 1.0
    local intensity_boost = 1.0
    if st.vel >= 0 then
      dir_boost = 1.0 + math.min(st.vel, 1.0) * 0.95
      intensity_boost = 0.85 + math.min(st.vel, 1.0) * 0.55
    else
      dir_boost = math.max(0.32, 0.65 + st.vel * 0.35)
      intensity_boost = math.max(0.28, 0.52 + st.vel * 0.25)
    end

    local target_spread = st.depth * max_spread * dir_boost
    local target_intensity = st.depth * intensity_boost
    local rate = math.min(1.0, dt * 16.0)
    st.smooth_spread = st.smooth_spread + (target_spread - st.smooth_spread) * rate
    st.smooth_intensity = st.smooth_intensity + (target_intensity - st.smooth_intensity) * rate

    if st.depth <= 0.01 and st.smooth_intensity <= 0.02 then
      active[k] = nil
    end
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local down_c = module.color("down_color", "#29e3ff")
  local up_c = module.color("up_color", "#8a6bff")

  for _, st in pairs(active) do
    if st.smooth_intensity > 0.015 then
      -- Blend color between vibrant downward press and calm upward release
      local mix_down = math.max(0.0, math.min(1.0, (st.vel + 0.6) / 1.2))
      local r = up_c[1] + (down_c[1] - up_c[1]) * mix_down
      local g = up_c[2] + (down_c[2] - up_c[2]) * mix_down
      local b = up_c[3] + (down_c[3] - up_c[3]) * mix_down
      ctx:key_glow(
        "glow",
        st.x,
        st.y,
        st.w,
        st.h,
        math.max(12.0, st.smooth_spread),
        4.0,
        math.min(1.35, st.smooth_intensity),
        { r, g, b, 0.92 }
      )
    end
  end
end
