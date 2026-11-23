-- Live QMK/Vial Keymap Legends Provider (`keys-keymap`, priority 10)
-- Subscribes directly to `keyboard/resolved_keys` (published live by QMK/Vial firmware on every
-- layer change or Vialite remap!) with graceful fallback to legacy `keyboard/keymap` / ANSI defaults.

local last_caps = false
local hw_keymap = nil
local resolved_keys = nil

-- USB HID Usage ID (0x04..0xE7) -> { base_label, optional_shifted, optional_icon }
local HID_MAP = {
  [0x1E] = { "1", "!" }, [0x1F] = { "2", "@" }, [0x20] = { "3", "#" }, [0x21] = { "4", "$" }, [0x22] = { "5", "%" },
  [0x23] = { "6", "^" }, [0x24] = { "7", "&" }, [0x25] = { "8", "*" }, [0x26] = { "9", "(" }, [0x27] = { "0", ")" },
  [0x28] = { "Enter" }, [0x29] = { "Esc" }, [0x2A] = { "Bksp" }, [0x2B] = { "Tab" }, [0x2C] = { "Space" },
  [0x2D] = { "-", "_" }, [0x2E] = { "=", "+" }, [0x2F] = { "[", "{" }, [0x30] = { "]", "}" }, [0x31] = { "\\", "|" },
  [0x32] = { "#", "~" }, [0x33] = { ";", ":" }, [0x34] = { "'", "\"" }, [0x35] = { "`", "~" },
  [0x36] = { ",", "<" }, [0x37] = { ".", ">" }, [0x38] = { "/", "?" }, [0x39] = { "Caps" },
  [0x3A] = { "F1" }, [0x3B] = { "F2" }, [0x3C] = { "F3" }, [0x3D] = { "F4" }, [0x3E] = { "F5" }, [0x3F] = { "F6" },
  [0x40] = { "F7" }, [0x41] = { "F8" }, [0x42] = { "F9" }, [0x43] = { "F10" }, [0x44] = { "F11" }, [0x45] = { "F12" },
  [0x46] = { "Prt" }, [0x47] = { "ScrLk" }, [0x48] = { "Pause" }, [0x49] = { "Ins" }, [0x4A] = { "Home" },
  [0x4B] = { "PgUp" }, [0x4C] = { "Del" }, [0x4D] = { "End" }, [0x4E] = { "PgDn" },
  [0x4F] = { "→" }, [0x50] = { "←" }, [0x51] = { "↓" }, [0x52] = { "↑" },
  [0x68] = { "F13" }, [0x69] = { "F14" }, [0x6A] = { "F15" }, [0x6B] = { "F16" },
  [0x6C] = { "F17" }, [0x6D] = { "F18" }, [0x6E] = { "F19" }, [0x6F] = { "F20" },
  -- QMK Consumer / Media keys
  [0xA8] = { "Mute", nil, "volume" }, [0xA9] = { "Vol+", nil, "volume" }, [0xAA] = { "Vol-", nil, "volume" },
  [0xAB] = { "Next", nil, "play" },   [0xAC] = { "Prev", nil, "play" },   [0xAD] = { "Stop", nil, "close" },
  [0xAE] = { "Play", nil, "play" },   [0xBD] = { "Bright", nil, "brightness" }, [0xBE] = { "Dim", nil, "brightness" },
  [0xC1] = { "Mission" }, [0xC2] = { "Launch" },
  -- Modifiers
  [0xE0] = { "Ctrl" }, [0xE1] = { "Shift" }, [0xE2] = { "Opt" }, [0xE3] = { "Cmd" },
  [0xE4] = { "Ctrl" }, [0xE5] = { "Shift" }, [0xE6] = { "Opt" }, [0xE7] = { "Cmd" },
}

-- Built-in Flux QK_KB_0..13 actions (`0x7E00..0x7E0D`, matching fw_qmk_keymap.c / vial.rs)
local QK_KB_LABELS = {
  [0]  = { "Bright", nil, "brightness" },
  [1]  = { "Dim", nil, "brightness" },
  [2]  = { "Prof+", nil, "star" },
  [3]  = { "Prof-", nil, "star" },
  [4]  = { "Act+" },
  [5]  = { "Act-" },
  [6]  = { "Linux" },
  [7]  = { "macOS" },
  [8]  = { "Win" },
  [9]  = { "Lock", nil, "power" },
  [10] = { "Pomo", nil, "play" },
  [11] = { "Reset", nil, "refresh" },
  [12] = { "Theme+", nil, "star" },
  [13] = { "Theme-", nil, "star" },
}

local function mod_short(m)
  local b = m & 0x0F
  if b == 1 then return "Ctl" end
  if b == 2 then return "Sft" end
  if b == 4 then return "Opt" end
  if b == 8 then return "Cmd" end
  return "Mod"
end

local function rule_from_qmk_keycode(ki, kc, src_layer, lower_default)
  if not kc or kc == 0 or kc == 1 then return nil end

  -- Standard letter A..Z (0x04..0x1D)
  if kc >= 0x04 and kc <= 0x1D then
    local ch = string.char(65 + (kc - 0x04))
    return {
      key = ki,
      label = lower_default and string.lower(ch) or ch,
      shift_case = lower_default,
      color = (src_layer and src_layer > 0) and "#7ae7ff" or nil,
    }
  end

  -- Standard HID / Consumer / Modifier keys
  if kc <= 0xFF then
    local m = HID_MAP[kc]
    if m then
      return {
        key = ki,
        label = m[1],
        shifted = m[2],
        icon = m[3],
        color = (src_layer and src_layer > 0) and "#ffd166" or nil,
      }
    end
    return { key = ki, label = string.format("0x%02X", kc) }
  end

  -- MT(mod, kc): 0x2000..0x3FFF
  if kc >= 0x2000 and kc <= 0x3FFF then
    local tap = kc & 0xFF
    local r = rule_from_qmk_keycode(ki, tap, src_layer, lower_default) or { key = ki, label = "Tap" }
    r.sub = mod_short((kc >> 8) & 0x1F)
    return r
  end

  -- LT(layer, kc): 0x4000..0x4FFF
  if kc >= 0x4000 and kc <= 0x4FFF then
    local tap = kc & 0xFF
    local lyr = (kc >> 8) & 0x0F
    local r = rule_from_qmk_keycode(ki, tap, src_layer, lower_default) or { key = ki, label = "Fn" }
    r.sub = "L" .. tostring(lyr)
    return r
  end

  -- MO(layer): 0x5220..0x523F
  if kc >= 0x5220 and kc <= 0x523F then
    local lyr = kc & 0x1F
    return { key = ki, label = (lyr == 1) and "Fn" or ("L" .. tostring(lyr)) }
  end
  -- DF / TG / OSL / TT layer keys
  if kc >= 0x5240 and kc <= 0x52FF then
    local lyr = kc & 0x1F
    local prefix = (kc < 0x5260) and "DF" or ((kc < 0x5280) and "TG" or ((kc < 0x52A0) and "OSL" or "TT"))
    return { key = ki, label = prefix .. tostring(lyr) }
  end
  -- OSM(mod): 0x5500..0x55FF
  if kc >= 0x5500 and kc <= 0x55FF then
    return { key = ki, label = "OS " .. mod_short(kc & 0x1F) }
  end

  -- QK_KB_0..31: 0x7E00..0x7E1F (Built-in Flux actions + dynamic module actions!)
  if kc >= 0x7E00 and kc <= 0x7E1F then
    local idx = kc - 0x7E00
    local m = QK_KB_LABELS[idx]
    if m then
      return { key = ki, label = m[1], shifted = m[2], icon = m[3] }
    end
    return { key = ki, label = "Act" .. tostring(idx) }
  end

  -- Caps Word / QMK specials
  if kc == 0x7C73 then return { key = ki, label = "CW" } end
  return { key = ki, label = string.format("0x%X", kc) }
end

local KC_LABELS = {
  KC_ESCAPE = { "Esc" }, KC_ESC = { "Esc" },
  KC_BACKSPACE = { "Bksp" }, KC_BSPC = { "Bksp" },
  KC_TAB = { "Tab" }, KC_CAPS = { "Caps" }, KC_CAPSLOCK = { "Caps" }, KC_CAPS_LOCK = { "Caps" },
  KC_ENTER = { "Enter" }, KC_ENT = { "Enter" },
  KC_LEFT_SHIFT = { "Shift" }, KC_RIGHT_SHIFT = { "Shift" }, KC_LSFT = { "Shift" }, KC_RSFT = { "Shift" },
  KC_LEFT_CTRL = { "Ctrl" }, KC_RIGHT_CTRL = { "Ctrl" }, KC_LCTL = { "Ctrl" }, KC_RCTL = { "Ctrl" },
  KC_LEFT_ALT = { "Opt" }, KC_RIGHT_ALT = { "Opt" }, KC_LALT = { "Opt" }, KC_RALT = { "Opt" },
  KC_LEFT_GUI = { "Cmd" }, KC_RIGHT_GUI = { "Cmd" }, KC_LGUI = { "Cmd" }, KC_RGUI = { "Cmd" },
  KC_SPACE = { "Space" }, KC_SPC = { "Space" },
  KC_GRAVE = { "`", "~" }, KC_GRV = { "`", "~" },
  KC_MINUS = { "-", "_" }, KC_MINS = { "-", "_" },
  KC_EQUAL = { "=", "+" }, KC_EQL = { "=", "+" },
  KC_LEFT_BRACKET = { "[", "{" }, KC_LBRACKET = { "[", "{" }, KC_LBRC = { "[", "{" },
  KC_RIGHT_BRACKET = { "]", "}" }, KC_RBRACKET = { "]", "}" }, KC_RBRC = { "]", "}" },
  KC_BACKSLASH = { "\\", "|" }, KC_BSLASH = { "\\", "|" }, KC_BSLS = { "\\", "|" },
  KC_SEMICOLON = { ";", ":" }, KC_SCOLON = { ";", ":" }, KC_SCLN = { ";", ":" },
  KC_QUOTE = { "'", "\"" }, KC_QUOT = { "'", "\"" },
  KC_COMMA = { ",", "<" }, KC_COMM = { ",", "<" },
  KC_DOT = { ".", ">" }, KC_SLASH = { "/", "?" }, KC_SLSH = { "/", "?" },
  KC_UP = { "↑" }, KC_DOWN = { "↓" }, KC_LEFT = { "←" }, KC_RIGHT = { "→" }, KC_RGHT = { "→" },
  KC_HOME = { "Home" }, KC_END = { "End" }, KC_DELETE = { "Del" }, KC_DEL = { "Del" },
  KC_INSERT = { "Ins" }, KC_INS = { "Ins" }, KC_PSCREEN = { "Prt" }, KC_PSCR = { "Prt" },
  KC_PAGE_UP = { "PgUp" }, KC_PGUP = { "PgUp" }, KC_PAGE_DOWN = { "PgDn" }, KC_PGDN = { "PgDn" },
  KC_1 = { "1", "!" }, KC_2 = { "2", "@" }, KC_3 = { "3", "#" }, KC_4 = { "4", "$" }, KC_5 = { "5", "%" },
  KC_6 = { "6", "^" }, KC_7 = { "7", "&" }, KC_8 = { "8", "*" }, KC_9 = { "9", "(" }, KC_0 = { "0", ")" },
  KC_OF_RESET_POMODORO = { "Reset", nil, "refresh" },
  KC_OF_START_PAUSE_POMODORO = { "Pomo", nil, "play" },
  KC_OF_BRIGHTNESS_UP = { "Bright", nil, "brightness" },
  KC_OF_BRIGHTNESS_DOWN = { "Dim", nil, "brightness" },
}

local function rule_from_legacy_action(ki, act, lower_default)
  if type(act) ~= "table" then return nil end
  if tonumber(act.type) == 2 or act.isMomentary == true or act.layerControlType == "momentaryLayer" then
    return { key = ki, label = "Fn" }
  end
  local kc = string.upper(tostring(act.keyCode or ""))
  if kc == "" or kc == "KC_NO" or kc == "KC_TRANSPARENT" then return nil end
  local mapped = KC_LABELS[kc]
  if mapped then
    return { key = ki, label = mapped[1], shifted = mapped[2], icon = mapped[3] }
  end
  if #kc == 4 and string.sub(kc, 1, 3) == "KC_" then
    local ch = string.sub(kc, 4, 4)
    if ch >= "A" and ch <= "Z" then
      return { key = ki, label = lower_default and string.lower(ch) or ch, shift_case = lower_default }
    end
  end
  if string.sub(kc, 1, 4) == "KC_F" and tonumber(string.sub(kc, 5)) then
    return { key = ki, label = string.sub(kc, 4) }
  end
  local leg = tostring(act.legend or "")
  if #leg > 0 then return { key = ki, label = leg } end
  return { key = ki, label = kc:gsub("^KC_", "") }
end

local function should_show_key(kc, src_layer, vis)
  if vis == "hidden" then
    return false
  elseif vis == "layers_only" then
    return (src_layer or 0) > 0
  elseif vis == "hide_letters" then
    -- Touch-typist mode: hide A..Z letters on Layer 0, keep symbols/numbers/modifiers/Fn layers!
    if (src_layer or 0) == 0 and kc >= 0x04 and kc <= 0x1D then
      return false
    end
    return true
  end
  return true
end

local function publish_keymap()
  local cfg = module.config()
  local lower_default = cfg.lowercase_default ~= false
  local vis = tostring(cfg.legend_visibility or "all")
  local rules = {}

  if vis == "hidden" then
    bus.publish("keys/providers/keymap", {
      priority = 10,
      mode = "replace",
      rules = {},
    }, true)
    return
  end

  -- Priority 1: Live QMK/Vial `keyboard/resolved_keys` (instant updates on every layer & Vialite edit!)
  if type(resolved_keys) == "table" and type(resolved_keys.keys) == "table" and #resolved_keys.keys > 0 then
    for idx, entry in ipairs(resolved_keys.keys) do
      if type(entry) == "table" then
        local kc = tonumber(entry.keycode) or 0
        local sl = tonumber(entry.source_layer) or 0
        if should_show_key(kc, sl, vis) then
          local r = rule_from_qmk_keycode(idx - 1, kc, sl, lower_default)
          if r then table.insert(rules, r) end
        end
      end
    end
  elseif type(hw_keymap) == "table" and type(hw_keymap.layers) == "table" and #hw_keymap.layers > 0 then
    if vis ~= "layers_only" then
      local l0 = hw_keymap.layers[1]
      if l0 and type(l0.keyActions) == "table" and #l0.keyActions > 0 then
        for idx, act in ipairs(l0.keyActions) do
          local r = rule_from_legacy_action(idx - 1, act, lower_default)
          if r then
            local skip = (vis == "hide_letters" and r.shift_case == true)
            if not skip then table.insert(rules, r) end
          end
        end
      end
    end
  end

  if #rules == 0 and vis == "all" then
    -- Minimal fallback ANSI default when keyboard is unplugged
    local fallback = {
      { 0, "Esc" }, { 1, "F1" }, { 2, "F2" }, { 3, "F3" }, { 4, "F4" }, { 5, "F5" }, { 6, "F6" },
      { 7, "F7" }, { 8, "F8" }, { 9, "F9" }, { 10, "F10" }, { 11, "F11" }, { 12, "F12" },
      { 13, "Prt" }, { 14, "Ins" }, { 15, "Del" }, { 29, "Bksp" }, { 31, "Tab" }, { 46, "Caps" },
      { 58, "Enter" }, { 60, "Shift" }, { 71, "Shift" }, { 74, "Ctrl" }, { 75, "Opt" }, { 76, "Cmd" },
      { 78, "Space" }, { 79, "Fn" }, { 82, "←" }, { 83, "↓" }, { 84, "→" },
    }
    for _, f in ipairs(fallback) do table.insert(rules, { key = f[1], label = f[2] }) end
  end

  bus.publish("keys/providers/keymap", {
    priority = 10,
    mode = "replace",
    rules = rules,
  }, true)
end

function on_init()
  local rk = bus.get("keyboard/resolved_keys")
  if type(rk) == "table" then resolved_keys = rk end
  local cur = bus.get("keyboard/keymap")
  if type(cur) == "table" then hw_keymap = cur end
  publish_keymap()
end

function on_event(topic, payload)
  if topic == "keyboard/resolved_keys" and type(payload) == "table" then
    resolved_keys = payload
    publish_keymap()
  elseif topic == "keyboard/keymap" and type(payload) == "table" then
    hw_keymap = payload
    if not resolved_keys then publish_keymap() end
  elseif topic == "keyboard/state" and type(payload) == "table" then
    local caps = (payload.caps_lock == true) or (payload.caps_word_active == true)
    if caps ~= last_caps then
      last_caps = caps
      if caps then
        bus.publish("keys/providers/keymap-caps", {
          priority = 90,
          mode = "badge",
          rules = { { key = 46, color = "#58f29c" } },
        }, true)
      else
        bus.publish("keys/providers/keymap-caps", nil, true)
      end
    end
  end
end
