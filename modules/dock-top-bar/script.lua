-- Top Bar Dock Module (`dock-top-bar`)
-- Owns flexible top-bar slot layout (supporting 0.5x half-slots, 1x, 2x, 3x, and 4x full-bar spans!),
-- GPU-smooth edge-free vertical fade vignette (ZERO horizontal lines or borders!),
-- and smart vertical dividers that only separate distinct widgets (never slicing through multi-slot widgets!).

local geom = nil
local computed_slots = {}
local occupancy = {}

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

local function is_boundary_crossed(boundary)
  for _, item in ipairs(occupancy) do
    local s = tonumber(item.start) or 0
    local sp = tonumber(item.span) or 1
    if s < boundary - 0.01 and (s + sp) > boundary + 0.01 then
      return true
    end
  end
  return false
end

function on_init()
  geom = keyboard.layout()
  recompute_slots()
  local occ = bus.get("layout/top_bar/occupancy")
  if type(occ) == "table" and type(occ.items) == "table" then
    occupancy = occ.items
  end
end

function on_event(topic, payload)
  if topic == "keyboard/geometry" and type(payload) == "table" then
    geom = payload
    recompute_slots()
  elseif topic == "layout/top_bar/occupancy" and type(payload) == "table" and type(payload.items) == "table" then
    occupancy = payload.items
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" or not geom or not geom.header_rect then return end
  local cfg = module.config()
  local dim_pct = tonumber(cfg.module_dimming) or 0
  -- True per-pixel GPU cubic vignette fade: ZERO horizontal lines, ZERO banding, ZERO sharp border!
  ctx:top_fade(math.max(0.0, math.min(0.85, dim_pct / 100.0)))

  if cfg.show_dividers ~= false and #computed_slots >= 2 then
    -- Draw full-slot dividers ONLY at integer boundaries not crossed by a multi-slot widget
    for i = 1, #computed_slots - 1 do
      if not is_boundary_crossed(i) then
        local a = computed_slots[i].rect
        local b = computed_slots[i + 1].rect
        local div_x = ((a.x + a.w) + b.x) * 0.5
        ctx:rect(div_x, a.y + 18, 1.2, math.max(20, a.h - 36), { 1.0, 1.0, 1.0, 0.14 })
      end
    end
    -- Subtle inner half-slot divider if two 0.5x widgets sit side-by-side inside a slot
    for _, item in ipairs(occupancy) do
      local s = tonumber(item.start) or 0
      local sp = tonumber(item.span) or 1
      local e = s + sp
      if math.abs((e - math.floor(e)) - 0.5) < 0.05 then
        local slot_idx = math.floor(e) + 1
        local slot_entry = computed_slots[slot_idx]
        if slot_entry then
          local sr = slot_entry.rect
          ctx:rect(sr.x + sr.w * 0.5, sr.y + 34, 1.0, math.max(16, sr.h - 68), { 1.0, 1.0, 1.0, 0.12 })
        end
      end
    end
  end
end
