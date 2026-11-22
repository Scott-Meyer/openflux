-- Top Bar Dock Module (`dock-top-bar`)
-- Owns top-bar slot layout (2, 3, or 4 slots), optional frosted dimming backdrop for
-- crisp widget legibility over bright backgrounds, and subtle vertical slot dividers.

local geom = nil
local computed_slots = {}

local function recompute_slots()
  if not geom or not geom.header_rect then return end
  local hr = geom.header_rect
  local count = tonumber(module.config().slot_count) or 4
  if count < 2 then count = 2 elseif count > 4 then count = 4 end
  local gap = (count == 4) and 89.4 or ((count == 3) and 56.0 or 40.0)
  local slot_w = (hr.w - (count - 1) * gap) / count

  computed_slots = {}
  for i = 0, count - 1 do
    local x = hr.x + i * (slot_w + gap)
    table.insert(computed_slots, {
      slot_index = i,
      rect = { x = x, y = hr.y, w = slot_w, h = hr.h },
      center_x = x + slot_w * 0.5,
      center_y = hr.y + hr.h * 0.5,
    })
  end

  bus.publish("layout/slots/top-bar", {
    owner = "dock-top-bar",
    slot_count = count,
    slots = computed_slots,
  }, true)
  bus.publish("layout/regions/dock-top-bar", {
    owner = "dock-top-bar",
    bounds = hr,
    slot_count = count,
  }, true)
end

function on_init()
  geom = keyboard.layout()
  recompute_slots()
end

function on_event(topic, payload)
  if topic == "keyboard/geometry" and type(payload) == "table" then
    geom = payload
    recompute_slots()
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" or not geom or not geom.header_rect then return end
  local cfg = module.config()
  local hr = geom.header_rect
  local dim_pct = tonumber(cfg.module_dimming) or 28
  if dim_pct > 1 then
    local alpha = math.min(0.85, (dim_pct / 100.0) * 0.85)
    ctx:round_rect(hr.x, hr.y, hr.w, hr.h, 14, { 0.028, 0.035, 0.055, alpha }, 0, { 0, 0, 0, 0 })
  end

  if cfg.show_dividers ~= false and #computed_slots >= 2 then
    for i = 1, #computed_slots - 1 do
      local a = computed_slots[i].rect
      local b = computed_slots[i + 1].rect
      local div_x = ((a.x + a.w) + b.x) * 0.5
      ctx:rect(div_x, a.y + 18, 1.2, math.max(20, a.h - 36), { 1.0, 1.0, 1.0, 0.16 })
    end
  end
end
