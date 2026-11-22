-- Base Keymap Provider (`keys-keymap`, priority 10)
-- Purely declarative: publishes base key legends, Shift case (a -> A), Shift symbols (1 -> !),
-- Fn-layer media/brightness legends, and Caps Lock indicator badge to `keys/providers/keymap`.

local last_caps = false

local function publish_keymap()
  local lower_default = module.config().lowercase_default ~= false
  local rules = {
    -- Row 0 (F-row)
    { key = 0,  label = "Esc" },
    { key = 1,  label = "F1" },  { key = 2,  label = "F2" },  { key = 3,  label = "F3" },
    { key = 4,  label = "F4" },  { key = 5,  label = "F5" },  { key = 6,  label = "F6" },
    { key = 7,  label = "F7" },  { key = 8,  label = "F8" },  { key = 9,  label = "F9" },
    { key = 10, label = "F10" }, { key = 11, label = "F11" }, { key = 12, label = "F12" },
    { key = 13, label = "Prt" }, { key = 14, label = "Ins" }, { key = 15, label = "Del" },

    -- Row 1 (Numbers + Shift symbols!)
    { key = 16, label = "`", shifted = "~" },
    { key = 17, label = "1", shifted = "!" }, { key = 18, label = "2", shifted = "@" },
    { key = 19, label = "3", shifted = "#" }, { key = 20, label = "4", shifted = "$" },
    { key = 21, label = "5", shifted = "%" }, { key = 22, label = "6", shifted = "^" },
    { key = 23, label = "7", shifted = "&" }, { key = 24, label = "8", shifted = "*" },
    { key = 25, label = "9", shifted = "(" }, { key = 26, label = "0", shifted = ")" },
    { key = 27, label = "-", shifted = "_" }, { key = 28, label = "=", shifted = "+" },
    { key = 29, label = "Bksp" }, { key = 30, label = "Home" },

    -- Row 2 (QWERTY with live Shift/Caps case!)
    { key = 31, label = "Tab" },
    { key = 32, label = "q", shift_case = lower_default }, { key = 33, label = "w", shift_case = lower_default },
    { key = 34, label = "e", shift_case = lower_default }, { key = 35, label = "r", shift_case = lower_default },
    { key = 36, label = "t", shift_case = lower_default }, { key = 37, label = "y", shift_case = lower_default },
    { key = 38, label = "u", shift_case = lower_default }, { key = 39, label = "i", shift_case = lower_default },
    { key = 40, label = "o", shift_case = lower_default }, { key = 41, label = "p", shift_case = lower_default },
    { key = 42, label = "[", shifted = "{" }, { key = 43, label = "]", shifted = "}" },
    { key = 44, label = "\\", shifted = "|" }, { key = 45, label = "PgUp" },

    -- Row 3 (ASDF)
    { key = 46, label = "Caps" },
    { key = 47, label = "a", shift_case = lower_default }, { key = 48, label = "s", shift_case = lower_default },
    { key = 49, label = "d", shift_case = lower_default }, { key = 50, label = "f", shift_case = lower_default },
    { key = 51, label = "g", shift_case = lower_default }, { key = 52, label = "h", shift_case = lower_default },
    { key = 53, label = "j", shift_case = lower_default }, { key = 54, label = "k", shift_case = lower_default },
    { key = 55, label = "l", shift_case = lower_default },
    { key = 56, label = ";", shifted = ":" }, { key = 57, label = "'", shifted = "\"" },
    { key = 58, label = "Enter" }, { key = 59, label = "PgDn" },

    -- Row 4 (ZXCV)
    { key = 60, label = "Shift" },
    { key = 61, label = "z", shift_case = lower_default }, { key = 62, label = "x", shift_case = lower_default },
    { key = 63, label = "c", shift_case = lower_default }, { key = 64, label = "v", shift_case = lower_default },
    { key = 65, label = "b", shift_case = lower_default }, { key = 66, label = "n", shift_case = lower_default },
    { key = 67, label = "m", shift_case = lower_default },
    { key = 68, label = ",", shifted = "<" }, { key = 69, label = ".", shifted = ">" },
    { key = 70, label = "/", shifted = "?" },
    { key = 71, label = "Shift" }, { key = 72, label = "↑" }, { key = 73, label = "End" },

    -- Row 5 (Modifiers & Arrows)
    { key = 74, label = "Ctrl" }, { key = 75, label = "Opt" }, { key = 76, label = "Cmd" },
    { key = 77, label = "Fn" },   { key = 78, label = "Space" },
    { key = 79, label = "Cmd" },  { key = 80, label = "Alt" }, { key = 81, label = "Ctrl" },
    { key = 82, label = "←" },    { key = 83, label = "↓" },   { key = 84, label = "→" },

    -- Fn Layer quick media & brightness icons (priority 20 when holding Fn!)
    { key = 1,  priority = 20, when = { fn = true }, icon = "brightness", label = "Dim",  color = "#ffd166" },
    { key = 2,  priority = 20, when = { fn = true }, icon = "brightness", label = "Bright", color = "#ffd166" },
    { key = 8,  priority = 20, when = { fn = true }, icon = "play",       label = "Play", color = "#4de1ff" },
    { key = 10, priority = 20, when = { fn = true }, icon = "volume",     label = "Mute", color = "#ff6b8b" },
    { key = 11, priority = 20, when = { fn = true }, icon = "volume",     label = "Vol-", color = "#4de1ff" },
    { key = 12, priority = 20, when = { fn = true }, icon = "volume",     label = "Vol+", color = "#4de1ff" },
  }

  if not lower_default then
    for _, r in ipairs(rules) do
      if r.label and #r.label == 1 and r.label >= "a" and r.label <= "z" then
        r.label = string.upper(r.label)
      end
    end
  end

  bus.publish("keys/providers/keymap", {
    priority = 10,
    mode = "replace",
    rules = rules,
  }, true)
end

function on_init()
  publish_keymap()
end

function on_event(topic, payload)
  if topic == "keyboard/state" and type(payload) == "table" then
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
