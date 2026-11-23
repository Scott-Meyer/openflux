-- Live Stream Background (`bg-stream`)
-- Stackable with `bg-media` and `bg-aurora-shader`!
-- When no stream is live/active, this module draws NOTHING (100% transparent) so your normal
-- background plays underneath untouched. The moment a YouTube / Twitch / HLS / RTSP stream
-- starts producing frames, it takes over the background (full 3D water ripples & key legends
-- continue working right on top!).

local stream = {
  live = false,
  local_path = "",
  kb_path = "/home/root/openflux/media/stream_live.jpg",
}

function on_init()
  local cur = bus.get("modules/bg-stream/state")
  if type(cur) == "table" then
    stream = cur
  end
end

function on_event(topic, payload)
  if topic == "modules/bg-stream/state" and type(payload) == "table" then
    stream = payload
  end
end

function on_render(ctx, stage)
  if stage ~= "background" then return end
  local cfg = module.config()
  if cfg.active == false or not stream.live then
    -- 100% transparent: let bg-media / bg-aurora-shader underneath show through!
    return
  end

  local is_linux_kb = (string.sub(module.dir(), 1, 5) == "/usr/" or string.sub(module.dir(), 1, 6) == "/home/")
  local path = is_linux_kb and tostring(stream.kb_path or "/home/root/openflux/media/stream_live.jpg")
    or tostring(stream.local_path or "")
  if #path == 0 then return end

  ctx:background_media(path)
  local dim = tonumber(cfg.bg_dimming) or 20
  if dim > 0 then
    ctx:background_dimming(math.max(0.0, math.min(0.85, dim / 100.0)))
  end
end
