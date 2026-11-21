-- Key-Bouncing & Key-Refracting Particles
-- Uses the compositor's physical Key Layout SDF physics so particles either bounce along the
-- 2mm inter-key trenches or glide beneath transparent keycaps with lens magnification.

local pending_bursts = {}
local seq = 1

function on_event(topic, payload)
  if topic == "input/keys/event" and payload.kind == "press" then
    table.insert(pending_bursts, { x = payload.x_px, y = payload.y_px, scale = 1.0 })
  elseif topic == "fx/surface/impulse" then
    table.insert(pending_bursts, { x = payload.x or 960, y = payload.y or 540, scale = (payload.force or 0.8) * 0.85 })
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  if #pending_bursts == 0 then return end

  local cfg = module.config()
  local count = math.floor(tonumber(cfg.count) or 14)
  local bounce = tonumber(cfg.bounce_chance) or 0.55
  local refract = tonumber(cfg.lens_refract) or 1.55
  local ca = module.color("color_a", "#ffd166")
  local cb = module.color("color_b", "#4df0ff")

  for _, b in ipairs(pending_bursts) do
    local n = math.max(4, math.floor(count * b.scale))
    for i = 1, n do
      seq = seq + 1
      local angle = (i / n) * math.pi * 2.0 + (seq % 7) * 0.17
      local speed = 220.0 + ((seq * 73) % 240)
      local mix_t = (i % 2 == 0) and 0.15 or 0.85
      local col = {
        ca[1] + (cb[1] - ca[1]) * mix_t,
        ca[2] + (cb[2] - ca[2]) * mix_t,
        ca[3] + (cb[3] - ca[3]) * mix_t,
        0.95,
      }
      ctx:spawn_particle({
        x = b.x + math.cos(angle) * 28.0,
        y = b.y + math.sin(angle) * 28.0,
        vx = math.cos(angle) * speed,
        vy = math.sin(angle) * speed,
        radius = 4.5 + (seq % 4),
        life_ms = 850.0 + (seq % 5) * 140.0,
        color = col,
        bounce_chance = bounce,
        lens_refract = refract,
        drag = 0.986,
      })
    end
  end
  pending_bursts = {}
end
