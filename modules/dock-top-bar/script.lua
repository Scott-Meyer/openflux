-- Top Bar Dock Module (`dock-top-bar`)
-- Owns flexible top-bar slot layout (supporting 0.5x half-slots, 1x, 2x, 3x, and 4x full-bar spans!),
-- seamless edge-free vertical fade vignette (no visible box edges!), and subtle slot dividers.

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
    header_rect = hr,
    gap = gap,
    slot_w = slot_w,
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
  local dim_pct = tonumber(cfg.module_dimming) or 0
  if dim_pct > 1 then
    -- Seamless top-down gradient fade (ZERO visible box edge or border!)
    local max_a = math.min(0.72, (dim_pct / 100.0) * 0.72)
    local fade_h = hr.h + 70
    local bands = 24
    local bh = fade_h / bands
    for i = 0, bands - 1 do
      local t = i / (bands - 1)
      local ease = (1.0 - t) * (1.0 - t)
      local a = max_a * ease
      if a > 0.005 then
        ctx:rect(0, i * bh, 1920, bh + 1, { 0.015, 0.02, 0.035, a })
      end
    end
  end

  if cfg.show_dividers ~= false and #computed_slots >= 2 then
    for i = 1, #computed_slots - 1 do
      local a = computed_slots[i].rect
      local b = computed_slots[i + 1].rect
      local div_x = ((a.x + a.w) + b.x) * 0.5
      ctx:rect(div_x, a.y + 18, 1.2, math.max(20, a.h - 36), { 1.0, 1.0, 1.0, 0.14 })
    end
  end
end
