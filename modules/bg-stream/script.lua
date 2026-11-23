-- Live Stream Background (`bg-stream`)
-- Stackable with `bg-media` and `bg-aurora-shader`!
-- Uses OpenFlux v2's `CH_MEDIA` live H.264 hardware stream (`ctx:stream_background("bg-stream")`)
-- decoded directly by the RK3566 `rkmpp` hardware VPU at up to 1080p 60 FPS with ZERO disk writes!
-- When no stream is active/live (or stream closes/offline), draws NOTHING (100% transparent)
-- so your ocean waves / Patagonia background underneath plays seamlessly untouched.

local stream_active = false

function on_init()
  local cur = bus.get("modules/streams/bg-stream")
  if type(cur) == "table" then
    stream_active = (cur.active == true)
  end
end

function on_event(topic, payload)
  if topic == "modules/streams/bg-stream" and type(payload) == "table" then
    stream_active = (payload.active == true)
  end
end

function on_render(ctx, stage)
  if stage ~= "background" then return end
  local cfg = module.config()
  if cfg.active == false or not stream_active then
    -- 100% transparent: underlying `bg-media` / `bg-aurora-shader` shines right through!
    return
  end

  ctx:stream_background("bg-stream")
  local dim = tonumber(cfg.bg_dimming) or 20
  if dim > 0 then
    ctx:background_dimming(math.max(0.0, math.min(0.85, dim / 100.0)))
  end
end
