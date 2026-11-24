-- Key-Bouncing & Refracting Particles: emit the bursts; the compositor handles the
-- inner key walls, optical magnification, and the floor.

local pending_bursts = {}
local held_keys = {}
local accents = {
  { 1.00, 0.43, 0.65, 0.95 }, -- pink
  { 0.73, 0.52, 1.00, 0.95 }, -- violet
  { 0.50, 1.00, 0.65, 0.95 }, -- mint
  { 1.00, 0.56, 0.31, 0.95 }, -- orange
  { 0.30, 0.53, 1.00, 0.95 }, -- blue
  { 1.00, 0.24, 0.30, 0.95 }, -- red
  { 0.78, 1.00, 0.32, 0.95 }, -- lime
  { 0.96, 0.95, 1.00, 0.95 }, -- pearl
}

local function launch_angle()
  -- Choose a 90° sector by weight, then pick a random direction within it.
  local pick = math.random()
  local sector = pick < 0.7 and -1 or pick < 0.8 and 0 or pick < 0.9 and 2 or 1
  return (sector + math.random() - 0.5) * (math.pi * 0.5), sector
end

local function burst_count(average)
  average = math.max(1, math.min(20, average))
  local whole = math.floor(average)
  local variation = math.min(1, whole - 1, 20 - math.ceil(average))
  local round_up = math.random() < average - whole and 1 or 0
  return whole + round_up + math.random(-variation, variation)
end

function on_event(topic, payload)
  if topic == "input/keys/event" then
    local id = payload.key_index
    if payload.kind == "press" and payload.x_px and payload.y_px then
      table.insert(pending_bursts, { x = payload.x_px, y = payload.y_px, scale = 1.0, from_key = true })
      if id ~= nil then
        held_keys[id] = { x = payload.x_px, y = payload.y_px, scale = 0.55, from_key = true }
      end
    elseif payload.kind == "release" and id ~= nil then
      held_keys[id] = nil
    end
  elseif topic == "fx/surface/impulse" then
    table.insert(pending_bursts, { x = payload.x or 960, y = payload.y or 540, scale = (payload.force or 0.8) * 0.85 })
  end
end

function on_update(_, now_ms)
  -- Stagger held bursts, and never catch up on bursts missed during a slow frame.
  local budget = 4
  for _, key in pairs(held_keys) do
    if not key.next_burst_ms then
      key.next_burst_ms = now_ms + 350
    elseif now_ms >= key.next_burst_ms and budget > 0 then
      table.insert(pending_bursts, key)
      budget = budget - 1
      key.next_burst_ms = now_ms + math.random(425, 500)
    end
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  if #pending_bursts == 0 then return end

  local cfg = module.config()
  local count = math.max(1, math.min(20, tonumber(cfg.count) or 3.5))
  local bounce = tonumber(cfg.bounce_chance) or 0.8
  local refract = tonumber(cfg.lens_refract) or 1.55
  local ca = module.color("color_a", "#ffd166")
  local cb = module.color("color_b", "#4df0ff")
  local colors = { { ca[1], ca[2], ca[3], 0.95 }, { cb[1], cb[2], cb[3], 0.95 }, table.unpack(accents) }

  for _, b in ipairs(pending_bursts) do
    for _ = 1, burst_count(count * b.scale) do
      local angle, sector = launch_angle()
      local speed = math.random(220, 459)
      local offset = math.random(20, 36)
      local dx, dy = math.cos(angle), math.sin(angle)
      local vy = dy * speed
      if b.from_key and sector == -1 then
        -- Give the upward fan enough lift to crest about 2–3 key rows above the source,
        -- including its small launch offset. Sideways speed stays unchanged.
        vy = -math.random(750, 925)
      elseif b.from_key and vy < 0 then
        vy = vy * 1.5
      end
      ctx:spawn_particle({
        x = b.x + dx * offset,
        y = b.y + dy * offset,
        vx = dx * speed,
        vy = vy,
        radius = 7.0 + math.random() * 3.0,
        -- Give the sparks time to fall, settle, and spill before the final fade.
        life_ms = math.random(6800, 8800),
        color = colors[math.random(#colors)],
        bounce_chance = bounce,
        lens_refract = refract,
        drag = 0.9985,
      })
    end
  end
  pending_bursts = {}
end
