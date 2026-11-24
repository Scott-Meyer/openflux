-- Analog Gamepad Controller Module (`analog-gamepad`)
--   1. Key bindings (`GP_LS_UP..GP_GUIDE`, 0x7E27..0x7E3F) live natively inside QMK / Vial layers.
--   2. This module owns the 3-way USB output mode (`off` | `hid` | `xinput`), deadzone/saturation/curve
--      tuning pushed via retained `gamepad/config` -> RP2040 `FW_CMD_SET_GAMEPAD_CONFIG (0x74)`,
--      and App Linking (`host/app/active` -> `keyboard/layer_override` -> `0x75`).

local active_mode = nil
local last_linked_match = false
local last_applied_layer = nil

local function publish_config(override_mode, notify)
  local cfg = module.config()
  local mode = tostring(override_mode or cfg.mode or "off")
  if mode ~= "off" and mode ~= "hid" and mode ~= "xinput" then
    mode = "off"
  end
  active_mode = mode

  local dz_mm = math.max(0.05, math.min(1.0, tonumber(cfg.deadzone_mm) or 0.15))
  local sat_mm = math.max(dz_mm + 0.3, math.min(2.8, tonumber(cfg.saturation_mm) or 2.6))
  local curve = tostring(cfg.curve or "s_curve")
  local radial = (cfg.radial_clamp ~= false)

  bus.publish("gamepad/config", {
    mode = mode,
    deadzone_um = math.floor(dz_mm * 1000 + 0.5),
    saturation_um = math.floor(sat_mm * 1000 + 0.5),
    curve = curve,
    radial_clamp = radial,
  }, true)

  if notify then
    local mode_label = (mode == "hid") and "Classic HID (macOS / Steam)"
      or ((mode == "xinput") and "XInput (Xbox / Windows)" or "Off (No Gamepad)")
    bus.publish("ui/toast", {
      id = "analog-gamepad-mode",
      icon = "star",
      title = "Analog Gamepad: " .. string.upper(mode),
      subtitle = mode_label,
      color = (mode == "off") and "#94a3b8" or "#58f29c",
      duration_ms = 2600,
    }, false)
  end
end

local function matches_linked_app(app_payload)
  local cfg = module.config()
  local raw = tostring(cfg.linked_apps or "")
  if #raw == 0 or type(app_payload) ~= "table" then
    return false, ""
  end
  local hay = string.lower(
    tostring(app_payload.process_name or "") .. " " ..
    tostring(app_payload.app_id or "") .. " " ..
    tostring(app_payload.window_title or "")
  )
  if #hay <= 2 then return false, "" end
  for tok in string.gmatch(raw, "[^,]+") do
    local t = string.lower((tok:match("^%s*(.-)%s*$") or ""))
    if #t >= 2 and string.find(hay, t, 1, true) then
      return true, tostring(app_payload.process_name or tok)
    end
  end
  return false, ""
end

local function apply_app_linking(app_payload)
  local matched, app_name = matches_linked_app(app_payload)
  if matched == last_linked_match then return end
  last_linked_match = matched
  local cfg = module.config()
  local target_layer = math.max(0, math.min(7, math.floor(tonumber(cfg.linked_layer) or 2)))
  if matched then
    last_applied_layer = target_layer
    bus.publish("keyboard/layer_override", { layer = target_layer, reason = app_name }, true)
    bus.publish("ui/toast", {
      id = "analog-gamepad-layer",
      icon = "star",
      title = "Gamepad Layer " .. tostring(target_layer),
      subtitle = "Linked to " .. app_name,
      color = "#58f29c",
      duration_ms = 2400,
    }, false)
  elseif last_applied_layer ~= nil then
    last_applied_layer = nil
    bus.publish("keyboard/layer_override", { layer = nil }, true)
  end
end

function on_init()
  publish_config(nil, false)
  local cur_app = bus.get("host/app/active")
  if type(cur_app) == "table" then
    apply_app_linking(cur_app)
  end
end

function on_event(topic, payload)
  if topic == "host/app/active" then
    apply_app_linking(payload)
  elseif topic == "modules/analog-gamepad/cycle_mode" then
    local cur = active_mode or tostring(module.config().mode or "off")
    local next_m = (cur == "off") and "hid" or ((cur == "hid") and "xinput" or "off")
    publish_config(next_m, true)
  end
end
