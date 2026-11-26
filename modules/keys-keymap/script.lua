-- Live QMK/Vial Keymap Legends Provider (`keys-keymap`, priority 10)
-- Subscribes directly to `keyboard/resolved_keys` (published live by QMK/Vial firmware on every
-- layer change or Vialite remap!) with graceful fallback to legacy `keyboard/keymap` / ANSI defaults.

local last_caps = false
local hw_keymap = nil
local resolved_keys = nil
local host_platform = "macos"
local active_profile = nil
local action_slots = {} -- numeric slot (0..95) -> system/actions entry

local function normalize_os(val)
  if type(val) ~= "string" then return nil end
  local s = string.lower(val)
  if s == "macos" or s == "darwin" or s == "osx" then return "macos" end
  if s == "windows" or s == "win" or s == "win32" then return "windows" end
  if s == "linux" then return "linux" end
  return nil
end

local function effective_os()
  local cfg = module.config()
  local explicit = normalize_os(cfg.os_layout)
  if explicit then return explicit end
  if type(active_profile) == "table" then
    local ov = active_profile.module_overrides
    if type(ov) == "table" and type(ov["keys-keymap"]) == "table" then
      local pov = normalize_os(ov["keys-keymap"].os_layout)
      if pov then return pov end
    end
    if type(active_profile.tags) == "table" then
      for _, t in ipairs(active_profile.tags) do
        local tos = normalize_os(t)
        if tos then return tos end
      end
    end
  end
  return normalize_os(host_platform) or "macos"
end

local function os_mod_entry(kind, os_name)
  if kind == "gui" then
    if os_name == "windows" then return { "Win", nil, "kbd_win" } end
    if os_name == "linux" then return { "Super", nil, "kbd_win" } end
    return { "Cmd", nil, "kbd_cmd" }
  elseif kind == "alt" then
    if os_name == "windows" or os_name == "linux" then return { "Alt", nil, nil } end
    return { "Opt", nil, "kbd_opt" }
  end
  return nil
end

local function index_system_actions(payload)
  action_slots = {}
  if type(payload) ~= "table" then return end
  local slots = payload.slots or payload
  if type(slots) ~= "table" then return end
  for k, entry in pairs(slots) do
    local s = tonumber(k) or (type(entry) == "table" and tonumber(entry.slot))
    if s and type(entry) == "table" then
      action_slots[math.floor(s)] = entry
    end
  end
end

local function resolve_action_image(entry, icon_str)
  if type(icon_str) ~= "string" or #icon_str == 0 then return nil end
  local lower = string.lower(icon_str)
  if not (lower:match("%.png$") or lower:match("%.jpg$") or lower:match("%.jpeg$") or lower:match("%.svg$")) then
    return nil
  end
  if string.sub(icon_str, 1, 1) == "/" or string.sub(icon_str, 1, 9) == "module://" or string.sub(icon_str, 1, 8) == "media://" then
    return icon_str
  end
  local mid = tostring(entry.module_id or "")
  if #mid > 0 then
    -- Portable cross-device asset ref resolved locally by `keys-renderer` on both Mac and keyboard!
    return "module://" .. mid .. "/" .. icon_str
  end
  return icon_str
end

-- USB HID Usage ID (0x04..0xE7) -> { base_label, optional_shifted, optional_icon }
local HID_MAP = {
  [0x1E] = { "1", "!" }, [0x1F] = { "2", "@" }, [0x20] = { "3", "#" }, [0x21] = { "4", "$" }, [0x22] = { "5", "%" },
  [0x23] = { "6", "^" }, [0x24] = { "7", "&" }, [0x25] = { "8", "*" }, [0x26] = { "9", "(" }, [0x27] = { "0", ")" },
  [0x28] = { "Enter", nil, "kbd_enter" }, [0x29] = { "Esc" }, [0x2A] = { "Bksp", nil, "kbd_bksp" },
  [0x2B] = { "Tab", nil, "kbd_tab" }, [0x2C] = { "", nil, "kbd_space" },
  [0x2D] = { "-", "_" }, [0x2E] = { "=", "+" }, [0x2F] = { "[", "{" }, [0x30] = { "]", "}" }, [0x31] = { "\\", "|" },
  [0x32] = { "#", "~" }, [0x33] = { ";", ":" }, [0x34] = { "'", "\"" }, [0x35] = { "`", "~" },
  [0x36] = { ",", "<" }, [0x37] = { ".", ">" }, [0x38] = { "/", "?" }, [0x39] = { "Caps", nil, "kbd_caps" },
  [0x3A] = { "F1" }, [0x3B] = { "F2" }, [0x3C] = { "F3" }, [0x3D] = { "F4" }, [0x3E] = { "F5" }, [0x3F] = { "F6" },
  [0x40] = { "F7" }, [0x41] = { "F8" }, [0x42] = { "F9" }, [0x43] = { "F10" }, [0x44] = { "F11" }, [0x45] = { "F12" },
  [0x46] = { "Prt" }, [0x47] = { "ScrLk" }, [0x48] = { "Pause" }, [0x49] = { "Ins" },
  [0x4A] = { "Home", nil, "kbd_home" }, [0x4B] = { "PgUp", nil, "kbd_pgup" }, [0x4C] = { "Del" },
  [0x4D] = { "End", nil, "kbd_end" }, [0x4E] = { "PgDn", nil, "kbd_pgdn" },
  [0x4F] = { "", nil, "kbd_right" }, [0x50] = { "", nil, "kbd_left" },
  [0x51] = { "", nil, "kbd_down" }, [0x52] = { "", nil, "kbd_up" },
  [0x68] = { "F13" }, [0x69] = { "F14" }, [0x6A] = { "F15" }, [0x6B] = { "F16" },
  [0x6C] = { "F17" }, [0x6D] = { "F18" }, [0x6E] = { "F19" }, [0x6F] = { "F20" },
  -- QMK Consumer / Media keys
  [0xA8] = { "Mute", nil, "volume" }, [0xA9] = { "Vol+", nil, "volume" }, [0xAA] = { "Vol-", nil, "volume" },
  [0xAB] = { "Next", nil, "play" },   [0xAC] = { "Prev", nil, "play" },   [0xAD] = { "Stop", nil, "close" },
  [0xAE] = { "Play", nil, "play" },   [0xBD] = { "Bright", nil, "brightness" }, [0xBE] = { "Dim", nil, "brightness" },
  [0xC1] = { "Mission" }, [0xC2] = { "Launch" },
  -- Modifiers (crisp icon + clean word caption, matching Fn & Reset!)
  [0xE0] = { "Ctrl", nil, "kbd_ctrl" }, [0xE1] = { "Shift", nil, "kbd_shift" },
  [0xE2] = { "Opt", nil, "kbd_opt" },   [0xE3] = { "Cmd", nil, "kbd_cmd" },
  [0xE4] = { "Ctrl", nil, "kbd_ctrl" }, [0xE5] = { "Shift", nil, "kbd_shift" },
  [0xE6] = { "Opt", nil, "kbd_opt" },   [0xE7] = { "Cmd", nil, "kbd_cmd" },
}

-- Fixed 25-slot Analog Gamepad keycodes (`0x7E27..0x7E3F`, slots 39..63 — matching fw_qmk_keymap.c / vial.rs)
local GP_LABELS = {
  [39] = "LS↑",   [40] = "LS↓",  [41] = "LS←",  [42] = "LS→",
  [43] = "RS↑",   [44] = "RS↓",  [45] = "RS←",  [46] = "RS→",
  [47] = "LT",    [48] = "RT",
  [49] = "A",     [50] = "B",    [51] = "X",    [52] = "Y",
  [53] = "LB",    [54] = "RB",   [55] = "L3",   [56] = "R3",
  [57] = "D↑",    [58] = "D↓",   [59] = "D←",   [60] = "D→",
  [61] = "Start", [62] = "Back", [63] = "Guide",
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

local function mod_short(m, os_name)
  local b = m & 0x0F
  if b == 15 then return "Hyp" end
  if b == 7 then return "Meh" end
  if os_name == "windows" or os_name == "linux" then
    if b == 1 then return "Ctrl" end
    if b == 2 then return "Shift" end
    if b == 4 then return "Alt" end
    if b == 8 then return os_name == "linux" and "Sup" or "Win" end
    return "Mod"
  end
  if b == 1 then return "⌃" end
  if b == 2 then return "⇧" end
  if b == 4 then return "⌥" end
  if b == 8 then return "⌘" end
  return "Mod"
end

local function mod_prefix_str(m, os_name)
  local b = m & 0x0F
  if b == 15 then return "Hyp+" end
  local s = ""
  if os_name == "windows" or os_name == "linux" then
    if (b & 1) ~= 0 then s = s .. "C+" end
    if (b & 4) ~= 0 then s = s .. "A+" end
    if (b & 8) ~= 0 then s = s .. (os_name == "linux" and "S+" or "W+") end
    if (b & 2) ~= 0 then s = s .. "Sh+" end
    return s
  end
  if (b & 1) ~= 0 then s = s .. "⌃" end
  if (b & 4) ~= 0 then s = s .. "⌥" end
  if (b & 8) ~= 0 then s = s .. "⌘" end
  if (b & 2) ~= 0 then s = s .. "⇧" end
  return s
end

local function rule_from_qmk_keycode(ki, kc, src_layer, lower_default, os_name)
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
    local m = nil
    if kc == 0xE2 or kc == 0xE6 then
      m = os_mod_entry("alt", os_name)
    elseif kc == 0xE3 or kc == 0xE7 then
      m = os_mod_entry("gui", os_name)
    else
      m = HID_MAP[kc]
    end
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

  -- Modified basic keycodes (0x0100..0x1FFF: S(kc), C(kc), A(kc), G(kc), C(S(kc)), etc.)
  if kc >= 0x0100 and kc <= 0x1FFF then
    local mods = (kc >> 8) & 0x1F
    local base = kc & 0xFF
    local col = (src_layer and src_layer > 0) and "#7ae7ff" or nil
    -- Pure Shift + symbol/number renders directly as the shifted glyph (`~`, `#`, `$`, `{`, `}`, `(`, `)`, `<`, `>`, `*`, `%`, `!`, `?`, `"`, `:`, `|`, `+`, `^`, `&`, `@`)!
    if (mods & 0x0F) == 2 then
      local hm = HID_MAP[base]
      if hm and hm[2] then
        return { key = ki, label = hm[2], color = col }
      end
      if base >= 0x04 and base <= 0x1D then
        return { key = ki, label = string.char(65 + (base - 0x04)), color = col }
      end
    end
    -- Special macOS Opt+[ / Opt+] smart quotes
    if os_name == "macos" and (mods & 0x0F) == 4 and base == 0x2F then return { key = ki, label = "“", color = col } end
    if os_name == "macos" and (mods & 0x0F) == 4 and base == 0x30 then return { key = ki, label = "‘", color = col } end
    -- Special Cmd+Ctrl+Shift+4 screenshot
    if os_name == "macos" and (mods & 0x0F) == 11 and base == 0x21 then return { key = ki, label = "Shot", icon = "cam", color = col } end
    local pfx = mod_prefix_str(mods, os_name)
    if base >= 0x04 and base <= 0x1D then
      return { key = ki, label = pfx .. string.char(65 + (base - 0x04)), color = col }
    end
    if base == 0x2B then return { key = ki, label = pfx .. "Tab", color = col } end
    local hm = HID_MAP[base]
    return { key = ki, label = pfx .. ((hm and #hm[1] > 0) and hm[1] or string.format("%X", base)), color = col }
  end

  -- MT(mod, kc): 0x2000..0x3FFF
  if kc >= 0x2000 and kc <= 0x3FFF then
    local tap = kc & 0xFF
    local r = rule_from_qmk_keycode(ki, tap, src_layer, lower_default, os_name) or { key = ki, label = "Tap" }
    r.sub = mod_short((kc >> 8) & 0x1F, os_name)
    return r
  end

  -- LT(layer, kc): 0x4000..0x4FFF
  if kc >= 0x4000 and kc <= 0x4FFF then
    local tap = kc & 0xFF
    local lyr = (kc >> 8) & 0x0F
    local r = rule_from_qmk_keycode(ki, tap, src_layer, lower_default, os_name) or { key = ki, label = "Fn" }
    r.sub = "L" .. tostring(lyr)
    return r
  end

  -- MO(layer): 0x5220..0x523F
  if kc >= 0x5220 and kc <= 0x523F then
    local lyr = kc & 0x1F
    return { key = ki, label = (lyr == 1 or lyr == 5 or lyr == 6) and "Fn" or ("L" .. tostring(lyr)), icon = "kbd_layer" }
  end
  -- DF / TG / OSL / TT layer keys
  if kc >= 0x5240 and kc <= 0x52FF then
    local lyr = kc & 0x1F
    local prefix = (kc < 0x5260) and "DF" or ((kc < 0x5280) and "TG" or ((kc < 0x52A0) and "OSL" or "TT"))
    return { key = ki, label = prefix .. tostring(lyr) }
  end
  -- OSM(mod): 0x5500..0x55FF
  if kc >= 0x5500 and kc <= 0x55FF then
    return { key = ki, label = "OS " .. mod_short(kc & 0x1F, os_name) }
  end
  -- TD(idx): 0x5700..0x57FF (Tap Dance)
  if kc >= 0x5700 and kc <= 0x57FF then
    local idx = kc - 0x5700
    local td_labels = { [0] = "‘/’", [1] = "“/”", [2] = "!/?" }
    return {
      key = ki,
      label = td_labels[idx] or ("TD" .. tostring(idx)),
      color = (src_layer and src_layer > 0) and "#7ae7ff" or nil,
    }
  end

  -- QK_KB_0..63 (`0x7E00..0x7E3F`) + QK_USER_0..31 (`0x7E40..0x7E5F`):
  -- Built-in Flux actions (0..13), dynamic module actions (14..38), and fixed Analog Gamepad (39..63)!
  if kc >= 0x7E00 and kc <= 0x7E5F then
    local idx = kc - 0x7E00
    if GP_LABELS[idx] then
      return { key = ki, label = GP_LABELS[idx], color = "#58f29c" }
    end
    local act = action_slots[idx]
    if act and idx >= 14 then
      local img = resolve_action_image(act, act.icon)
      local vec_icon = (not img and type(act.icon) == "string" and #act.icon > 0) and act.icon or nil
      local lbl = (type(act.short_label) == "string") and act.short_label or tostring(act.label or ("Act" .. tostring(idx)))
      local col = (type(act.accent) == "string" and #act.accent > 0) and act.accent or nil
      return {
        key = ki,
        label = lbl,
        icon = vec_icon,
        image = img,
        color = col,
      }
    end
    local m = QK_KB_LABELS[idx]
    if m then
      return { key = ki, label = m[1], shifted = m[2], icon = m[3] }
    end
    if act then
      return { key = ki, label = tostring(act.short_label or act.label or ("Act" .. tostring(idx))) }
    end
    return { key = ki, label = "Act" .. tostring(idx) }
  end

  -- Caps Word / QMK specials
  if kc == 0x7C73 then return { key = ki, label = "CW" } end
  return { key = ki, label = string.format("0x%X", kc) }
end

local KC_LABELS = {
  KC_ESCAPE = { "Esc" }, KC_ESC = { "Esc" },
  KC_BACKSPACE = { "Bksp", nil, "kbd_bksp" }, KC_BSPC = { "Bksp", nil, "kbd_bksp" },
  KC_TAB = { "Tab", nil, "kbd_tab" },
  KC_CAPS = { "Caps", nil, "kbd_caps" }, KC_CAPSLOCK = { "Caps", nil, "kbd_caps" }, KC_CAPS_LOCK = { "Caps", nil, "kbd_caps" },
  KC_ENTER = { "Enter", nil, "kbd_enter" }, KC_ENT = { "Enter", nil, "kbd_enter" },
  KC_LEFT_SHIFT = { "Shift", nil, "kbd_shift" }, KC_RIGHT_SHIFT = { "Shift", nil, "kbd_shift" },
  KC_LSFT = { "Shift", nil, "kbd_shift" }, KC_RSFT = { "Shift", nil, "kbd_shift" },
  KC_LEFT_CTRL = { "Ctrl", nil, "kbd_ctrl" }, KC_RIGHT_CTRL = { "Ctrl", nil, "kbd_ctrl" },
  KC_LCTL = { "Ctrl", nil, "kbd_ctrl" }, KC_RCTL = { "Ctrl", nil, "kbd_ctrl" },
  KC_LEFT_ALT = { "Opt", nil, "kbd_opt" }, KC_RIGHT_ALT = { "Opt", nil, "kbd_opt" },
  KC_LALT = { "Opt", nil, "kbd_opt" }, KC_RALT = { "Opt", nil, "kbd_opt" },
  KC_LEFT_GUI = { "Cmd", nil, "kbd_cmd" }, KC_RIGHT_GUI = { "Cmd", nil, "kbd_cmd" },
  KC_LGUI = { "Cmd", nil, "kbd_cmd" }, KC_RGUI = { "Cmd", nil, "kbd_cmd" },
  KC_SPACE = { "", nil, "kbd_space" }, KC_SPC = { "", nil, "kbd_space" },
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
  KC_UP = { "", nil, "kbd_up" }, KC_DOWN = { "", nil, "kbd_down" },
  KC_LEFT = { "", nil, "kbd_left" }, KC_RIGHT = { "", nil, "kbd_right" }, KC_RGHT = { "", nil, "kbd_right" },
  KC_HOME = { "Home", nil, "kbd_home" }, KC_END = { "End", nil, "kbd_end" },
  KC_DELETE = { "Del" }, KC_DEL = { "Del" },
  KC_INSERT = { "Ins" }, KC_INS = { "Ins" }, KC_PSCREEN = { "Prt" }, KC_PSCR = { "Prt" },
  KC_PAGE_UP = { "PgUp", nil, "kbd_pgup" }, KC_PGUP = { "PgUp", nil, "kbd_pgup" },
  KC_PAGE_DOWN = { "PgDn", nil, "kbd_pgdn" }, KC_PGDN = { "PgDn", nil, "kbd_pgdn" },
  KC_1 = { "1", "!" }, KC_2 = { "2", "@" }, KC_3 = { "3", "#" }, KC_4 = { "4", "$" }, KC_5 = { "5", "%" },
  KC_6 = { "6", "^" }, KC_7 = { "7", "&" }, KC_8 = { "8", "*" }, KC_9 = { "9", "(" }, KC_0 = { "0", ")" },
  KC_OF_RESET_POMODORO = { "Reset", nil, "refresh" },
  KC_OF_START_PAUSE_POMODORO = { "Pomo", nil, "play" },
  KC_OF_BRIGHTNESS_UP = { "Bright", nil, "brightness" },
  KC_OF_BRIGHTNESS_DOWN = { "Dim", nil, "brightness" },
}

local function rule_from_legacy_action(ki, act, lower_default, os_name)
  if type(act) ~= "table" then return nil end
  if tonumber(act.type) == 2 or act.isMomentary == true or act.layerControlType == "momentaryLayer" then
    return { key = ki, label = "Fn", icon = "kbd_layer" }
  end
  local kc = string.upper(tostring(act.keyCode or ""))
  if kc == "" or kc == "KC_NO" or kc == "KC_TRANSPARENT" then return nil end
  if kc == "KC_LEFT_ALT" or kc == "KC_RIGHT_ALT" or kc == "KC_LALT" or kc == "KC_RALT" then
    local m = os_mod_entry("alt", os_name)
    return { key = ki, label = m[1], shifted = m[2], icon = m[3] }
  end
  if kc == "KC_LEFT_GUI" or kc == "KC_RIGHT_GUI" or kc == "KC_LGUI" or kc == "KC_RGUI" then
    local m = os_mod_entry("gui", os_name)
    return { key = ki, label = m[1], shifted = m[2], icon = m[3] }
  end
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
  local os_name = effective_os()
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
          local r = rule_from_qmk_keycode(idx - 1, kc, sl, lower_default, os_name)
          if r then table.insert(rules, r) end
        end
      end
    end
  elseif type(hw_keymap) == "table" and type(hw_keymap.layers) == "table" and #hw_keymap.layers > 0 then
    if vis ~= "layers_only" then
      local l0 = hw_keymap.layers[1]
      if l0 and type(l0.keyActions) == "table" and #l0.keyActions > 0 then
        for idx, act in ipairs(l0.keyActions) do
          local r = rule_from_legacy_action(idx - 1, act, lower_default, os_name)
          if r then
            local skip = (vis == "hide_letters" and r.shift_case == true)
            if not skip then table.insert(rules, r) end
          end
        end
      end
    end
  end

  if #rules == 0 and vis == "all" then
    local gui_m = os_mod_entry("gui", os_name)
    local alt_m = os_mod_entry("alt", os_name)
    local mod_row = (os_name == "windows" or os_name == "linux") and {
      { 74, "Ctrl", "kbd_ctrl" }, { 75, gui_m[1], gui_m[3] }, { 76, alt_m[1], alt_m[3] }, { 77, alt_m[1], alt_m[3] },
    } or {
      { 74, gui_m[1], gui_m[3] }, { 75, alt_m[1], alt_m[3] }, { 76, "Ctrl", "kbd_ctrl" }, { 77, "Ctrl", "kbd_ctrl" },
    }
    -- Complete 85-key ANSI QWERTY fallback when keyboard is unplugged so Keyglass previews
    -- always show crisp, realistic keycap legends!
    local fallback = {
      { 0, "Esc" }, { 1, "F1" }, { 2, "F2" }, { 3, "F3" }, { 4, "F4" }, { 5, "F5" }, { 6, "F6" },
      { 7, "F7" }, { 8, "F8" }, { 9, "F9" }, { 10, "F10" }, { 11, "F11" }, { 12, "F12" },
      { 13, "Prt" }, { 14, "Ins" }, { 15, "Del" },
      { 16, "`", nil, "~" }, { 17, "1", nil, "!" }, { 18, "2", nil, "@" }, { 19, "3", nil, "#" },
      { 20, "4", nil, "$" }, { 21, "5", nil, "%" }, { 22, "6", nil, "^" }, { 23, "7", nil, "&" },
      { 24, "8", nil, "*" }, { 25, "9", nil, "(" }, { 26, "0", nil, ")" }, { 27, "-", nil, "_" },
      { 28, "=", nil, "+" }, { 29, "Bksp", "kbd_bksp" }, { 30, "Home", "kbd_home" },
      { 31, "Tab", "kbd_tab" },
      { 32, "q" }, { 33, "w" }, { 34, "e" }, { 35, "r" }, { 36, "t" },
      { 37, "y" }, { 38, "u" }, { 39, "i" }, { 40, "o" }, { 41, "p" },
      { 42, "[", nil, "{" }, { 43, "]", nil, "}" }, { 44, "\\", nil, "|" }, { 45, "End", "kbd_end" },
      { 46, "Caps", "kbd_caps" },
      { 47, "a" }, { 48, "s" }, { 49, "d" }, { 50, "f" }, { 51, "g" },
      { 52, "h" }, { 53, "j" }, { 54, "k" }, { 55, "l" }, { 56, ";", nil, ":" }, { 57, "'", nil, "\"" },
      { 58, "Enter", "kbd_enter" }, { 59, "PgUp", "kbd_pgup" },
      { 60, "Shift", "kbd_shift" },
      { 61, "z" }, { 62, "x" }, { 63, "c" }, { 64, "v" }, { 65, "b" }, { 66, "n" }, { 67, "m" },
      { 68, ",", nil, "<" }, { 69, ".", nil, ">" }, { 70, "/", nil, "?" },
      { 71, "Shift", "kbd_shift" }, { 72, "", "kbd_up" }, { 73, "PgDn", "kbd_pgdn" },
      mod_row[1], mod_row[2], mod_row[3], mod_row[4],
      { 78, "", "kbd_space" }, { 79, "Fn", "kbd_layer" },
      { 80, "Reset", "refresh" }, { 81, "Pomo", "play" },
      { 82, "", "kbd_left" }, { 83, "", "kbd_down" }, { 84, "", "kbd_right" },
    }
    for _, f in ipairs(fallback) do
      local is_letter = (#f[2] == 1 and f[2] >= "a" and f[2] <= "z")
      local lbl = is_letter and (lower_default and f[2] or string.upper(f[2])) or f[2]
      table.insert(rules, { key = f[1], label = lbl, icon = f[3], shifted = f[4], shift_case = is_letter })
    end
  end

  bus.publish("keys/providers/keymap", {
    priority = 10,
    mode = "replace",
    rules = rules,
  }, true)
end

function on_init()
  local hc = bus.get("host/connection")
  if type(hc) == "table" and type(hc.host_platform) == "string" and #hc.host_platform > 0 then
    host_platform = hc.host_platform
  end
  local ap = bus.get("profiles/active")
  if type(ap) == "table" then active_profile = ap end
  local sa = bus.get("system/actions")
  if type(sa) == "table" then index_system_actions(sa) end
  local rk = bus.get("keyboard/resolved_keys")
  if type(rk) == "table" then resolved_keys = rk end
  local cur = bus.get("keyboard/keymap")
  if type(cur) == "table" then hw_keymap = cur end
  publish_keymap()
end

function on_event(topic, payload)
  if topic == "host/connection" and type(payload) == "table" then
    if type(payload.host_platform) == "string" and #payload.host_platform > 0 and payload.host_platform ~= host_platform then
      host_platform = payload.host_platform
      publish_keymap()
    end
  elseif topic == "profiles/active" and type(payload) == "table" then
    active_profile = payload
    publish_keymap()
  elseif topic == "system/actions" and type(payload) == "table" then
    index_system_actions(payload)
    publish_keymap()
  elseif topic == "keyboard/resolved_keys" and type(payload) == "table" then
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
