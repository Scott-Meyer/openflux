-- Base Keymap Provider (`keys-keymap`, priority 10)
-- Purely declarative: publishes base key legends, Shift case (a -> A), Shift symbols (1 -> !),
-- Fn-layer media/brightness legends, and Caps Lock indicator badge to `keys/providers/keymap`.

local last_caps = false
local hw_keymap = nil

local KC_LABELS = {
  KC_ESCAPE = { "Esc" }, KC_ESC = { "Esc" },
  KC_BACKSPACE = { "Bksp" }, KC_BSPC = { "Bksp" },
  KC_TAB = { "Tab" }, KC_CAPS = { "Caps" }, KC_CAPSLOCK = { "Caps" },
  KC_ENTER = { "Enter" }, KC_ENT = { "Enter" },
  KC_LEFT_SHIFT = { "Shift" }, KC_RIGHT_SHIFT = { "Shift" }, KC_LSFT = { "Shift" }, KC_RSFT = { "Shift" },
  KC_LEFT_CTRL = { "Ctrl" }, KC_RIGHT_CTRL = { "Ctrl" }, KC_LCTL = { "Ctrl" }, KC_RCTL = { "Ctrl" },
  KC_LEFT_ALT = { "Opt" }, KC_RIGHT_ALT = { "Opt" }, KC_LALT = { "Opt" }, KC_RALT = { "Opt" },
  KC_LEFT_GUI = { "Cmd" }, KC_RIGHT_GUI = { "Cmd" }, KC_LGUI = { "Cmd" }, KC_RGUI = { "Cmd" },
  KC_SPACE = { "Space" }, KC_SPC = { "Space" },
  KC_GRAVE = { "`", "~" }, KC_GRV = { "`", "~" },
  KC_MINUS = { "-", "_" }, KC_MINS = { "-", "_" },
  KC_EQUAL = { "=", "+" }, KC_EQL = { "=", "+" },
  KC_LBRACKET = { "[", "{" }, KC_LBRC = { "[", "{" },
  KC_RBRACKET = { "]", "}" }, KC_RBRC = { "]", "}" },
  KC_BSLASH = { "\\", "|" }, KC_BSLS = { "\\", "|" },
  KC_SCOLON = { ";", ":" }, KC_SCLN = { ";", ":" },
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

local function rule_from_action(ki, act, lower_default)
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
  -- Single letter KC_A .. KC_Z
  if #kc == 4 and string.sub(kc, 1, 3) == "KC_" then
    local ch = string.sub(kc, 4, 4)
    if ch >= "A" and ch <= "Z" then
      return { key = ki, label = lower_default and string.lower(ch) or ch, shift_case = lower_default }
    end
  end
  -- Function keys KC_F1..KC_F24
  if string.sub(kc, 1, 4) == "KC_F" and tonumber(string.sub(kc, 5)) then
    return { key = ki, label = string.sub(kc, 4) }
  end
  local leg = tostring(act.legend or "")
  if #leg > 0 then
    return { key = ki, label = leg }
  end
  return { key = ki, label = kc:gsub("^KC_", "") }
end

local function publish_keymap()
  local lower_default = module.config().lowercase_default ~= false
  local rules = {}

  if type(hw_keymap) == "table" and type(hw_keymap.layers) == "table" and #hw_keymap.layers > 0 then
    local l0 = hw_keymap.layers[1]
    if l0 and type(l0.keyActions) == "table" and #l0.keyActions > 0 then
      for idx, act in ipairs(l0.keyActions) do
        local r = rule_from_action(idx - 1, act, lower_default)
        if r then table.insert(rules, r) end
      end
    end
  end

  if #rules == 0 then
    rules = {
      -- Fallback ANSI default when no hardware keymap has arrived yet
      { key = 0,  label = "Esc" },
      { key = 1,  label = "F1" },  { key = 2,  label = "F2" },  { key = 3,  label = "F3" },
      { key = 4,  label = "F4" },  { key = 5,  label = "F5" },  { key = 6,  label = "F6" },
      { key = 7,  label = "F7" },  { key = 8,  label = "F8" },  { key = 9,  label = "F9" },
      { key = 10, label = "F10" }, { key = 11, label = "F11" }, { key = 12, label = "F12" },
      { key = 13, label = "Prt" }, { key = 14, label = "Ins" }, { key = 15, label = "Del" },
      { key = 16, label = "`", shifted = "~" },
      { key = 17, label = "1", shifted = "!" }, { key = 18, label = "2", shifted = "@" },
      { key = 19, label = "3", shifted = "#" }, { key = 20, label = "4", shifted = "$" },
      { key = 21, label = "5", shifted = "%" }, { key = 22, label = "6", shifted = "^" },
      { key = 23, label = "7", shifted = "&" }, { key = 24, label = "8", shifted = "*" },
      { key = 25, label = "9", shifted = "(" }, { key = 26, label = "0", shifted = ")" },
      { key = 27, label = "-", shifted = "_" }, { key = 28, label = "=", shifted = "+" },
      { key = 29, label = "Bksp" }, { key = 30, label = "Home" },
      { key = 31, label = "Tab" },
      { key = 32, label = "q", shift_case = lower_default }, { key = 33, label = "w", shift_case = lower_default },
      { key = 34, label = "e", shift_case = lower_default }, { key = 35, label = "r", shift_case = lower_default },
      { key = 36, label = "t", shift_case = lower_default }, { key = 37, label = "y", shift_case = lower_default },
      { key = 38, label = "u", shift_case = lower_default }, { key = 39, label = "i", shift_case = lower_default },
      { key = 40, label = "o", shift_case = lower_default }, { key = 41, label = "p", shift_case = lower_default },
      { key = 42, label = "[", shifted = "{" }, { key = 43, label = "]", shifted = "}" },
      { key = 44, label = "\\", shifted = "|" }, { key = 45, label = "PgUp" },
      { key = 46, label = "Caps" },
      { key = 47, label = "a", shift_case = lower_default }, { key = 48, label = "s", shift_case = lower_default },
      { key = 49, label = "d", shift_case = lower_default }, { key = 50, label = "f", shift_case = lower_default },
      { key = 51, label = "g", shift_case = lower_default }, { key = 52, label = "h", shift_case = lower_default },
      { key = 53, label = "j", shift_case = lower_default }, { key = 54, label = "k", shift_case = lower_default },
      { key = 55, label = "l", shift_case = lower_default },
      { key = 56, label = ";", shifted = ":" }, { key = 57, label = "'", shifted = "\"" },
      { key = 58, label = "Enter" }, { key = 59, label = "PgDn" },
      { key = 60, label = "Shift" },
      { key = 61, label = "z", shift_case = lower_default }, { key = 62, label = "x", shift_case = lower_default },
      { key = 63, label = "c", shift_case = lower_default }, { key = 64, label = "v", shift_case = lower_default },
      { key = 65, label = "b", shift_case = lower_default }, { key = 66, label = "n", shift_case = lower_default },
      { key = 67, label = "m", shift_case = lower_default },
      { key = 68, label = ",", shifted = "<" }, { key = 69, label = ".", shifted = ">" },
      { key = 70, label = "/", shifted = "?" },
      { key = 71, label = "Shift" }, { key = 72, label = "↑" }, { key = 73, label = "End" },
      { key = 74, label = "Cmd" }, { key = 75, label = "Opt" }, { key = 76, label = "Ctrl" },
      { key = 77, label = "Ctrl" }, { key = 78, label = "Space" },
      { key = 79, label = "Fn" }, { key = 80, label = "Reset", icon = "refresh" }, { key = 81, label = "Pomo", icon = "play" },
      { key = 82, label = "←" }, { key = 83, label = "↓" }, { key = 84, label = "→" },
    }
  end

  -- Fn layer quick media & brightness icons (priority 20 when holding Fn!)
  table.insert(rules, { key = 1,  priority = 20, when = { fn = true }, icon = "brightness", label = "Dim",    color = "#ffd166" })
  table.insert(rules, { key = 2,  priority = 20, when = { fn = true }, icon = "brightness", label = "Bright", color = "#ffd166" })
  table.insert(rules, { key = 8,  priority = 20, when = { fn = true }, icon = "play",       label = "Play",   color = "#4de1ff" })
  table.insert(rules, { key = 10, priority = 20, when = { fn = true }, icon = "volume",     label = "Mute",   color = "#ff6b8b" })
  table.insert(rules, { key = 11, priority = 20, when = { fn = true }, icon = "volume",     label = "Vol-",   color = "#4de1ff" })
  table.insert(rules, { key = 12, priority = 20, when = { fn = true }, icon = "volume",     label = "Vol+",   color = "#4de1ff" })

  bus.publish("keys/providers/keymap", {
    priority = 10,
    mode = "replace",
    rules = rules,
  }, true)
end

function on_init()
  local cur = bus.get("keyboard/keymap")
  if type(cur) == "table" then hw_keymap = cur end
  publish_keymap()
end

function on_event(topic, payload)
  if topic == "keyboard/keymap" and type(payload) == "table" then
    hw_keymap = payload
    publish_keymap()
  elseif topic == "keyboard/state" and type(payload) == "table" then
    local caps = payload.caps_lock == true
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
