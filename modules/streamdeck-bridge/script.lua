-- Stream Deck Layer-Aware Keycap Renderer (`streamdeck-bridge`, priority 42)
-- Projects live Stream Deck button faces (dynamic PNG icons from `.sdPlugin` `setImage`,
-- multi-state icons from `setState`, live `setTitle`, and alert/ok badges) ONLY onto physical
-- keys whose CURRENTLY ACTIVE resolved QMK keycode is bound to that Stream Deck button!
--
-- Switching QMK layers (Base <-> Fn <-> Layer 2 <-> Layer 3) immediately swaps Stream Deck
-- keycaps with ZERO hardcoded "ability bar" and ZERO ghost icons!

local resolved_keys = nil
local action_by_kc = {} -- numeric QMK keycode -> button_id
local faces = {}        -- button_id -> { title, image, color, border, alert, ok }

local function index_actions(payload)
  action_by_kc = {}
  if type(payload) ~= "table" then return end
  local slots = payload.slots or payload
  if type(slots) ~= "table" then return end
  for _, a in pairs(slots) do
    if type(a) == "table" and tonumber(a.keycode) and a.module_id == "streamdeck-bridge" then
      local aid = tostring(a.action_id or "")
      local bid = aid:gsub("^streamdeck%-bridge[.:]", "")
      if #bid > 0 then
        action_by_kc[math.floor(tonumber(a.keycode))] = {
          button_id = bid,
          default_title = tostring(a.short_label or a.label or ""),
          default_icon = type(a.icon) == "string" and a.icon or nil,
          default_color = type(a.accent) == "string" and a.accent or "#5ce1ff",
        }
      end
    end
  end
end

local function publish_streamdeck_keys()
  if type(resolved_keys) ~= "table" or type(resolved_keys.keys) ~= "table" then
    bus.publish("keys/providers/streamdeck", nil, true)
    return
  end

  local rules = {}
  for idx, entry in ipairs(resolved_keys.keys) do
    if type(entry) == "table" then
      local kc = math.floor(tonumber(entry.keycode) or 0)
      local meta = action_by_kc[kc]
      if meta then
        local face = faces[meta.button_id]
        local lbl = (face and type(face.title) == "string") and face.title or meta.default_title
        local img = (face and type(face.image) == "string" and #face.image > 0) and face.image or meta.default_icon
        local col = (face and type(face.color) == "string" and #face.color > 0) and face.color or meta.default_color
        local border = col
        if face and face.alert == true then
          border = "#ff5c6c"
        elseif face and face.ok == true then
          border = "#58f29c"
        end
        table.insert(rules, {
          key = idx - 1,
          label = lbl,
          image = img,
          color = col,
          border = border,
          tile_bg = true,
        })
      end
    end
  end

  if #rules == 0 then
    bus.publish("keys/providers/streamdeck", nil, true)
  else
    bus.publish("keys/providers/streamdeck", {
      priority = 42,
      mode = "replace",
      rules = rules,
    }, true)
  end
end

function on_init()
  local sa = bus.get("system/actions")
  if type(sa) == "table" then index_actions(sa) end
  local rk = bus.get("keyboard/resolved_keys")
  if type(rk) == "table" then resolved_keys = rk end
  local f = bus.get("modules/streamdeck/faces")
  if type(f) == "table" then faces = f end
  publish_streamdeck_keys()
end

function on_event(topic, payload)
  if topic == "system/actions" and type(payload) == "table" then
    index_actions(payload)
    publish_streamdeck_keys()
  elseif topic == "keyboard/resolved_keys" and type(payload) == "table" then
    resolved_keys = payload
    publish_streamdeck_keys()
  elseif topic == "modules/streamdeck/faces" and type(payload) == "table" then
    faces = payload
    publish_streamdeck_keys()
  end
end
