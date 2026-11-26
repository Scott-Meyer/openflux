-- Key Rules (`keys-custom`): the user's own key-face rules.
--
-- Turns the `rules` list setting (plus the older per-key / per-action overrides that Studio's
-- Keycaps tab writes) into `patch` rules for `keys-renderer` on `keys/providers/custom`.
-- Rules sit in the user band (priority 100+), above every module's faces; later rows win ties.
-- What a rule can do is documented in docs/design/key-faces.md.

local USER_BAND = 100

local function resolve_media_path(raw)
  if type(raw) ~= "string" or #raw == 0 then return nil end
  if string.sub(raw, 1, 1) == "/" or string.find(raw, "://", 1, true) then return raw end
  local mdir = module.dir()
  if string.sub(mdir, 1, 5) == "/usr/" or string.sub(mdir, 1, 6) == "/home/" then
    return "/home/root/openflux/media/" .. raw
  end
  local home = os.getenv("HOME")
  if home and #home > 0 then
    return home .. "/.local/share/openflux/media/" .. raw
  end
  return raw
end

local function trim(s) return (tostring(s or ""):gsub("^%s+", ""):gsub("%s+$", "")) end
local function str(v) local s = trim(v); return #s > 0 and s or nil end

local GROUPS = { all = true, fkeys = true, numbers = true, letters = true, home_row = true, mods = true, arrows = true, nav = true }

-- "F1-F4, Esc, a, home_row, row:2, action:mqtt-bridge.scene_on, 47" -> list of selectors.
local function parse_keys(spec)
  local keys, sels = {}, {}
  for tok in string.gmatch(tostring(spec or ""), "[^,]+") do
    tok = trim(tok)
    local low = string.lower(tok)
    local fa, fb = string.match(low, "^f(%d+)%s*%-%s*f(%d+)$")
    if fa and fb then
      for n = tonumber(fa), tonumber(fb) do table.insert(keys, "KC_F" .. n) end
    elseif GROUPS[low] or string.match(low, "^row:%d+$") then
      table.insert(sels, { group = low })
    elseif string.sub(low, 1, 7) == "action:" then
      table.insert(sels, { action = trim(string.sub(tok, 8)) })
    elseif tonumber(tok) then
      table.insert(keys, math.floor(tonumber(tok)))
    elseif #tok > 0 then
      table.insert(keys, (string.sub(string.upper(tok), 1, 3) == "KC_") and string.upper(tok) or tok)
    end
  end
  if #keys > 0 then table.insert(sels, { keys = keys }) end
  return sels
end

local function corner_item(v)
  local s = str(v)
  if not s then return nil end
  if s == "dot" or s == "•" then return { dot = true } end
  if string.sub(s, 1, 5) == "icon:" then return { icon = trim(string.sub(s, 6)) } end
  return { text = s }
end

-- One row of the `rules` setting -> the look (`set`) and conditions (`when`) it describes.
local function row_to_parts(row)
  local set, when = {}, {}
  local hide = str(row.hide)
  if hide == "legend" or hide == "all" then set.hide = hide end
  if str(row.label) then set.label = row.label end
  if str(row.icon) then set.icon = row.icon end
  if str(row.image) then set.image = resolve_media_path(row.image) end
  if str(row.color) then set.color = row.color end
  local glow = str(row.glow)
  if glow and glow ~= "none" then
    set.glow = { color = str(row.glow_color), pulse = glow == "pulse", strength = glow == "subtle" and 0.5 or 1.0 }
  end
  local outline = str(row.outline)
  if outline and outline ~= "none" then
    set.outline = { shape = outline, color = str(row.outline_color), pulse = row.outline_pulse == true }
  end
  if str(row.backdrop) then set.backdrop = { color = row.backdrop, alpha = 0.45 } end
  local corners = {}
  for _, slot in ipairs({ "tl", "tr", "bl", "br" }) do
    local v = row["corner_" .. slot]
    if str(v) == "none" then corners[slot] = false else corners[slot] = corner_item(v) end
  end
  if next(corners) then set.corners = corners end

  local layer = str(row.layer)
  if layer and layer ~= "any" and tonumber(layer) then when.layer = tonumber(layer) end
  if str(row.app) then when.app = row.app end
  local held = str(row.held)
  if held == "held" then when.held = true elseif held == "not_held" then when.held = false end
  if str(row.caps) == "on" then when.caps = true elseif str(row.caps) == "off" then when.caps = false end
  if str(row.time) then when.time = row.time end
  local idle_s = tonumber(row.idle_seconds)
  if idle_s and idle_s > 0 then when.idle_ms = math.floor(idle_s * 1000) end
  local topic = str(row.condition_topic)
  if topic then
    local op = str(row.condition_op) or "eq"
    local c = { topic = topic, path = str(row.condition_path) }
    local val = row.condition_value
    if op == "exists" then c.exists = true
    elseif op == "missing" then c.exists = false
    elseif op == "in" then
      local list = {}
      for v in string.gmatch(tostring(val or ""), "[^,]+") do table.insert(list, trim(v)) end
      c["in"] = list
    else c[op] = val end
    when.bus = c
  end
  return set, next(when) and when or nil
end

local function emit(rules, sels, set, when, prio)
  for _, sel in ipairs(sels) do
    local r = { priority = prio, when = when }
    for k, v in pairs(sel) do r[k] = v end
    for k, v in pairs(set) do r[k] = v end
    table.insert(rules, r)
  end
end

-- Older Keycaps-tab overrides: label/color/icon/image/hide, plus bg -> backdrop, border -> outline.
local function legacy_set(ov)
  if type(ov) ~= "table" then return nil end
  local set = {}
  if str(ov.label) then set.label = ov.label end
  if str(ov.color) then set.color = ov.color end
  if str(ov.icon) then set.icon = ov.icon end
  if str(ov.image) then set.image = resolve_media_path(ov.image) end
  if str(ov.bg) then set.backdrop = { color = ov.bg, alpha = 0.45 } end
  if str(ov.border) then set.outline = { shape = "window", color = ov.border } end
  if ov.hide == true then set.hide = "legend" end
  return next(set) and set or nil
end

local function publish_rules()
  local cfg = module.config()
  local rules = {}

  -- Legacy overrides first (lowest in the user band): per action, then per key.
  local by_action = type(cfg.by_action) == "table" and cfg.by_action or {}
  for action_id, ov in pairs(by_action) do
    local set = legacy_set(ov)
    if set then emit(rules, { { action = action_id } }, set, nil, USER_BAND) end
  end
  local by_key = type(cfg.by_key) == "table" and cfg.by_key or {}
  for kstr, ov in pairs(by_key) do
    local lyr_s, idx_s = tostring(kstr):match("^([^:]+):(%d+)$")
    local set = legacy_set(ov)
    if set and idx_s then
      local when = (lyr_s ~= "*" and tonumber(lyr_s)) and { layer = tonumber(lyr_s) } or nil
      emit(rules, { { key = tonumber(idx_s) } }, set, when, USER_BAND + 1)
    end
  end

  -- The rules list, in order: a later row wins over an earlier one.
  local list = cfg.rules
  if type(list) == "string" then list = json.decode(list) end
  if type(list) == "table" then
    for i, row in ipairs(list) do
      if type(row) == "table" and row.enabled ~= false then
        local sels = parse_keys(row.keys)
        local set, when = row_to_parts(row)
        if #sels > 0 and next(set) then emit(rules, sels, set, when, USER_BAND + 10 + i) end
      end
    end
  end

  if #rules == 0 then
    bus.publish("keys/providers/custom", nil, true)
  else
    bus.publish("keys/providers/custom", { priority = USER_BAND, mode = "patch", rules = rules }, true)
  end
end

function on_init()
  publish_rules()
end

function on_event(topic, payload)
  -- Config changes reload the module; nothing else to react to.
end
