local lit = {}

function on_event(topic, payload)
  if topic == "input/keys/event" then
    local k = payload.key_index
    if payload.kind == "press" then
      lit[k] = { x = payload.x_px, y = payload.y_px, w = payload.w_px, h = payload.h_px, held = true, fade = 1.0 }
    elseif lit[k] then
      lit[k].held = false
    end
  end
end

function on_update(dt)
  for k, st in pairs(lit) do
    if not st.held then
      st.fade = st.fade - dt * 2.4
      if st.fade <= 0.01 then lit[k] = nil end
    end
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local c = module.color("color", "#ff8c42")
  for _, st in pairs(lit) do
    ctx:key_glow("solid", st.x, st.y, st.w, st.h, 12.0, 2.0, st.fade * 0.72, { c[1], c[2], c[3], st.fade })
  end
end
