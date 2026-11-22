-- StreamDeck-Style Live Ability Deck (`fx-ability-deck`, priority 50)
-- Purely declarative: publishes all three modifier banks (Base, Shift, Ctrl) with live
-- cooldowns, charges, and Ultimate % straight to `keys/providers/ability-deck`!
-- `keys-renderer` resolves them atomically in 0ms—100% transparent over the background
-- with zero ghosting of underlying letters/shortcuts and automatic LWT cleanup when disabled.

local shift_held = false
local ctrl_held = false
local dirty = true
local pub_accum = 0.0

local KEY_LSHIFT, KEY_RSHIFT = 60, 71
local KEY_LCTRL, KEY_RCTRL = 74, 81

local decks = {
  base = {
    [17] = { name = "FIRE",   icon = "flame",  color = "#ff5c33", cd = 0.0, max_cd = 4.0,  charges = 0, max_charges = 0 },
    [18] = { name = "FROST",  icon = "snow",   color = "#45d9ff", cd = 2.4, max_cd = 6.0,  charges = 2, max_charges = 2 },
    [19] = { name = "BLINK",  icon = "bolt",   color = "#a86bff", cd = 0.0, max_cd = 8.0,  charges = 2, max_charges = 3 },
    [20] = { name = "SHIELD", icon = "shield", color = "#4dff9e", cd = 5.1, max_cd = 10.0, charges = 0, max_charges = 0 },
    [21] = { name = "METEOR", icon = "star",   color = "#ff9f2e", cd = 9.5, max_cd = 14.0, charges = 0, max_charges = 0 },
    [22] = { name = "POTION", icon = "heart",  color = "#ff4f79", cd = 0.0, max_cd = 12.0, charges = 4, max_charges = 5 },
    [32] = { name = "SLASH",  icon = "sword",  color = "#f2c94c", cd = 0.0, max_cd = 2.5,  charges = 0, max_charges = 0 },
    [34] = { name = "DASH",   icon = "arrow",  color = "#3ce8c6", cd = 1.6, max_cd = 5.0,  charges = 1, max_charges = 2 },
    [35] = { name = "ULT",    icon = "crown",  color = "#ffd166", cd = 0.0, max_cd = 20.0, charges = 0, max_charges = 0, ult_pct = 92 },
    [50] = { name = "PARRY",  icon = "ring",   color = "#68b5ff", cd = 0.0, max_cd = 3.5,  charges = 0, max_charges = 0 },
  },
  shift = {
    [17] = { name = "MIC",    icon = "mic",    color = "#ff5964", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = true },
    [18] = { name = "DEAFEN", icon = "ring",   color = "#ff8c42", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = false },
    [19] = { name = "CLIP",   icon = "cam",    color = "#4de1ff", cd = 0.0, max_cd = 5.0,  charges = 0, max_charges = 0 },
    [20] = { name = "SCENE1", icon = "cam",    color = "#7c6bff", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = true },
    [21] = { name = "SCENE2", icon = "cam",    color = "#4dffb8", cd = 0.0, max_cd = 1.0,  charges = 0, max_charges = 0, toggled = false },
    [22] = { name = "LIVE",   icon = "star",   color = "#ff3b5c", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0, toggled = true },
    [32] = { name = "PING",   icon = "bolt",   color = "#ffd166", cd = 0.0, max_cd = 3.0,  charges = 3, max_charges = 3 },
    [34] = { name = "MOUNT",  icon = "arrow",  color = "#68d8ff", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0 },
    [35] = { name = "RECALL", icon = "ring",   color = "#c084fc", cd = 0.0, max_cd = 8.0,  charges = 0, max_charges = 0 },
    [50] = { name = "LOOT",   icon = "crown",  color = "#58f29c", cd = 0.0, max_cd = 1.5,  charges = 0, max_charges = 0 },
  },
  ctrl = {
    [17] = { name = "BUILD",  icon = "bolt",   color = "#4de1ff", cd = 0.0, max_cd = 4.0,  charges = 0, max_charges = 0 },
    [18] = { name = "TEST",   icon = "shield", color = "#58f29c", cd = 0.0, max_cd = 5.0,  charges = 0, max_charges = 0 },
    [19] = { name = "SYNC",   icon = "arrow",  color = "#a86bff", cd = 0.0, max_cd = 3.0,  charges = 0, max_charges = 0 },
    [20] = { name = "DEPLOY", icon = "flame",  color = "#ff9f2e", cd = 0.0, max_cd = 12.0, charges = 0, max_charges = 0 },
    [21] = { name = "LOGS",   icon = "ring",   color = "#68b5ff", cd = 0.0, max_cd = 1.5,  charges = 0, max_charges = 0 },
    [22] = { name = "KILL",   icon = "star",   color = "#ff4f64", cd = 0.0, max_cd = 3.0,  charges = 0, max_charges = 0 },
    [32] = { name = "GIT",    icon = "arrow",  color = "#ff7849", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0 },
    [34] = { name = "DOCKER", icon = "shield", color = "#38bdf8", cd = 0.0, max_cd = 4.0,  charges = 0, max_charges = 0 },
    [35] = { name = "BENCH",  icon = "crown",  color = "#ffd166", cd = 0.0, max_cd = 6.0,  charges = 0, max_charges = 0 },
    [50] = { name = "PRUSA",  icon = "cam",    color = "#fa6831", cd = 0.0, max_cd = 2.0,  charges = 0, max_charges = 0 },
  },
}

local function publish_provider()
  local cfg = module.config()
  local show_plate = cfg.tile_bg ~= false
  local rules = {}
  local bank_mods = { base = 0, shift = 2, ctrl = 1 }
  for bname, mod_bits in pairs(bank_mods) do
    for ki, ab in pairs(decks[bname]) do
      table.insert(rules, {
        key = ki,
        mods = mod_bits,
        label = ab.name,
        icon = ab.icon,
        color = ab.color,
        cd = ab.cd,
        max_cd = ab.max_cd,
        charges = ab.charges,
        max_charges = ab.max_charges,
        ult_pct = ab.ult_pct,
        toggled = ab.toggled,
        tile_bg = show_plate,
      })
    end
  end
  -- Clean Shift/Ctrl bank indicator right on the held modifier key (replaces "Shift"/"Ctrl" transparently!)
  table.insert(rules, { key = KEY_LSHIFT, mods = 2, label = "SHIFT DECK", color = "#4de1ff", tile_bg = true })
  table.insert(rules, { key = KEY_LCTRL,  mods = 1, label = "CTRL DECK",  color = "#ff9f2e", tile_bg = true })

  local when = {}
  if type(cfg.app_filter) == "string" and #cfg.app_filter > 0 then when.app = cfg.app_filter end
  if type(cfg.profile_filter) == "string" and #cfg.profile_filter > 0 then when.profile = cfg.profile_filter end

  bus.publish("keys/providers/ability-deck", {
    priority = 50,
    mode = "replace",
    when = when,
    rules = rules,
  }, true)
  dirty = false
end

function on_init()
  publish_provider()
end

function on_event(topic, payload)
  if topic == "input/keys/event" and type(payload) == "table" then
    local ki = payload.key_index
    local down = (payload.kind == "press")
    if ki == KEY_LSHIFT or ki == KEY_RSHIFT then
      shift_held = down
      return
    elseif ki == KEY_LCTRL or ki == KEY_RCTRL then
      ctrl_held = down
      return
    end
    if down then
      local bname = ctrl_held and "ctrl" or (shift_held and "shift" or "base")
      local ab = decks[bname] and decks[bname][ki]
      if ab then
        if ab.toggled ~= nil then
          ab.toggled = not ab.toggled
        elseif ab.ult_pct ~= nil then
          if ab.ult_pct >= 100 then
            ab.ult_pct = 0
            ab.cd = ab.max_cd
            bus.publish("fx/surface/impulse", { x = payload.x_px, y = payload.y_px, radius = 95, force = 1.4, kind = "shockwave", source = "module:fx-ability-deck" }, false)
          end
        elseif ab.max_charges > 0 then
          if ab.charges > 0 then
            ab.charges = ab.charges - 1
            if ab.cd <= 0 then ab.cd = ab.max_cd end
          end
        elseif ab.cd <= 0 then
          ab.cd = ab.max_cd
        end
        dirty = true
        bus.publish("modules/ability-deck/cast", { deck = bname, key_index = ki, ability = ab.name }, false)
      end
    end
  elseif topic == "modules/ability-deck/state" and type(payload) == "table" and type(payload.decks) == "table" then
    for dname, dmap in pairs(payload.decks) do
      if decks[dname] and type(dmap) == "table" then
        for kstr, info in pairs(dmap) do
          local ki = tonumber(kstr)
          if ki and decks[dname][ki] and type(info) == "table" then
            for fk, fv in pairs(info) do decks[dname][ki][fk] = fv end
            dirty = true
          end
        end
      end
    end
  end
end

function on_update(dt)
  local ticking = false
  for _, deck in pairs(decks) do
    for _, ab in pairs(deck) do
      if ab.cd and ab.cd > 0 then
        ab.cd = math.max(0, ab.cd - dt)
        ticking = true
        if ab.cd == 0 and ab.max_charges > 0 and ab.charges < ab.max_charges then
          ab.charges = ab.charges + 1
          if ab.charges < ab.max_charges then ab.cd = ab.max_cd end
        end
      end
      if ab.ult_pct ~= nil and ab.ult_pct < 100 and (ab.cd or 0) <= 0 then
        ab.ult_pct = math.min(100, ab.ult_pct + dt * 4.5)
        ticking = true
      end
    end
  end
  pub_accum = pub_accum + dt
  if dirty or (ticking and pub_accum >= 0.12) then
    pub_accum = 0.0
    publish_provider()
  end
end
