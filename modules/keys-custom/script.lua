-- Custom Keycap Visual Overrides Provider (`keys-custom`, priority 80)
-- Layers user-configured per-action AND per-key visual overrides cleanly above `keys-keymap` [10]
-- and `keys-shortcuts` [30], projecting dynamically across whatever key/layer an action is bound to!

local resolved_keys = nil
local action_by_kc = {} -- numeric QMK keycode -> action entry { action_id, ... }

local function resolve_media_path(raw)
  if type(raw) ~= "string" or #raw == 0 then return nil end
  if string.sub(raw, 1, 1) == "/" then return raw end
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

local function index_actions(payload)
  action_by_kc = {}
  if type(payload) ~= "table" then return end
  local list = payload.actions or payload
  if type(list) ~= "table" then return end
  for _, a in pairs(list) do
    if type(a) == "table" and tonumber(a.keycode) then
      action_by_kc[math.floor(tonumber(a.keycode))] = a
    end
  end
end

local function make_rule(ki, ov, prio)
  if type(ov) ~= "table" then return nil end
  local r = { key = ki, priority = prio }
  local any = false
  if type(ov.label) == "string" and #ov.label > 0 then r.label = ov.label; any = true end
  if type(ov.color) == "string" and #ov.color > 0 then r.color = ov.color; any = true end
  if type(ov.bg) == "string" and #ov.bg > 0 then r.bg = ov.bg; any = true end
  if type(ov.border) == "string" and #ov.border > 0 then r.border = ov.border; any = true end
  if type(ov.icon) == "string" and #ov.icon > 0 then r.icon = ov.icon; any = true end
  if type(ov.image) == "string" and #ov.image > 0 then
    r.image = resolve_media_path(ov.image)
    any = true
  end
  if ov.hide == true then
    r.label = ""
    r.icon = nil
    r.image = nil
    any = true
  end
  if not any then return nil end
  return r
end

local function publish_overrides()
  local cfg = module.config()
  local by_action = type(cfg.by_action) == "table" and cfg.by_action or {}
  local by_key = type(cfg.by_key) == "table" and cfg.by_key or {}
  local rules = {}

  -- 1. Per-action overrides (priority 78): follows the action onto ANY key/layer where it is bound!
  if type(resolved_keys) == "table" and type(resolved_keys.keys) == "table" then
    for idx, entry in ipairs(resolved_keys.keys) do
      if type(entry) == "table" then
        local kc = math.floor(tonumber(entry.keycode) or 0)
        local act = action_by_kc[kc]
        if act and type(act.action_id) == "string" then
          local ov = by_action[act.action_id]
          local r = make_rule(idx - 1, ov, 78)
          if r then table.insert(rules, r) end
        end
      end
    end
  end

  -- 2. Per-key overrides (priority 82): keyed by "<layer>:<key_index>" or "*:<key_index>"!
  local active_layer = 0
  if type(resolved_keys) == "table" and tonumber(resolved_keys.active_layer) then
    active_layer = math.floor(tonumber(resolved_keys.active_layer))
  end
  for kstr, ov in pairs(by_key) do
    local lyr_s, idx_s = tostring(kstr):match("^([^:]+):(%d+)$")
    if lyr_s and idx_s then
      local ki = tonumber(idx_s)
      if ki and (lyr_s == "*" or tonumber(lyr_s) == active_layer) then
        local r = make_rule(ki, ov, 82)
        if r then table.insert(rules, r) end
      end
    end
  end

  if #rules == 0 then
    bus.publish("keys/providers/custom", nil, true)
  else
    bus.publish("keys/providers/custom", {
      priority = 80,
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
  publish_overrides()
end

function on_event(topic, payload)
  if topic == "system/actions" and type(payload) == "table" then
    index_actions(payload)
    publish_overrides()
  elseif topic == "keyboard/resolved_keys" and type(payload) == "table" then
    resolved_keys = payload
    publish_overrides()
  end
end
