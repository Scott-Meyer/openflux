-- Modular Keycap Face Renderer (`keys-renderer`)
-- Evaluates all retained `keys/providers/#` declarations (keymap, shortcuts, game decks, badges)
-- against live held modifiers, Caps Lock, Fn layer, foreground app, and active profile inside
-- ONE single atomic frame—zero 1-frame flicker, 100% transparent over video/ripples, and
-- automatically aligned to each key's optical parallax (`visual_x`, `visual_y`).

local geom = nil
local providers = {} -- topic -> provider table (nil tombstone removes it on Erlang/LWT purge!)
local held = { ctrl = false, shift = false, alt = false, cmd = false, fn = false }
local held_by_idx = {} -- physical key_index -> true while held (prevents dual-Shift/dual-Ctrl glitches!)
local caps_lock = false
local active_layer = 0
local active_app = { app_id = "", process_name = "", window_title = "", profile_name = "" }
local active_profile = { index = 0, id = "", name = "" }
local anim_t = 0.0

-- Logical keyCode -> ANSI physical key_index lookup (declared BEFORE apply_hardware_keymap!)
local KEYCODE_TO_INDEX = {
  KC_ESC = 0, ESC = 0, KC_F1 = 1, F1 = 1, KC_F2 = 2, F2 = 2, KC_F3 = 3, F3 = 3,
  KC_F4 = 4, F4 = 4, KC_F5 = 5, F5 = 5, KC_F6 = 6, F6 = 6, KC_F7 = 7, F7 = 7,
  KC_F8 = 8, F8 = 8, KC_F9 = 9, F9 = 9, KC_F10 = 10, F10 = 10, KC_F11 = 11, F11 = 11,
  KC_F12 = 12, F12 = 12, KC_PSCR = 13, PRT = 13, KC_INS = 14, INS = 14, KC_DEL = 15, DEL = 15,
  KC_GRV = 16, ["`"] = 16, KC_1 = 17, ["1"] = 17, KC_2 = 18, ["2"] = 18, KC_3 = 19, ["3"] = 19,
  KC_4 = 20, ["4"] = 20, KC_5 = 21, ["5"] = 21, KC_6 = 22, ["6"] = 22, KC_7 = 23, ["7"] = 23,
  KC_8 = 24, ["8"] = 24, KC_9 = 25, ["9"] = 25, KC_0 = 26, ["0"] = 26,
  KC_MINS = 27, ["-"] = 27, KC_EQL = 28, ["="] = 28, KC_BSPC = 29, BKSP = 29, KC_HOME = 30, HOME = 30,
  KC_TAB = 31, TAB = 31, KC_Q = 32, Q = 32, KC_W = 33, W = 33, KC_E = 34, E = 34,
  KC_R = 35, R = 35, KC_T = 36, T = 36, KC_Y = 37, Y = 37, KC_U = 38, U = 38,
  KC_I = 39, I = 39, KC_O = 40, O = 40, KC_P = 41, P = 41,
  KC_LBRC = 42, ["["] = 42, KC_RBRC = 43, ["]"] = 43, KC_BSLS = 44, ["\\"] = 44, KC_PGUP = 45, PGUP = 45,
  KC_CAPS = 46, CAPS = 46, KC_A = 47, A = 47, KC_S = 48, S = 48, KC_D = 49, D = 49,
  KC_F = 50, F = 50, KC_G = 51, G = 51, KC_H = 52, H = 52, KC_J = 53, J = 53,
  KC_K = 54, K = 54, KC_L = 55, L = 55, KC_SCLN = 56, [";"] = 56, KC_QUOT = 57, ["'"] = 57,
  KC_ENT = 58, ENTER = 58, KC_PGDN = 59, PGDN = 59,
  KC_LSFT = 60, KC_Z = 61, Z = 61, KC_X = 62, X = 62, KC_C = 63, C = 63,
  KC_V = 64, V = 64, KC_B = 65, B = 65, KC_N = 66, N = 66, KC_M = 67, M = 67,
  KC_COMM = 68, [","] = 68, KC_DOT = 69, ["."] = 69, KC_SLSH = 70, ["/"] = 70,
  KC_RSFT = 71, KC_UP = 72, UP = 72, KC_END = 73, END = 73,
  KC_LCTL = 74, KC_LALT = 75, KC_LGUI = 76, KC_SPC = 78, SPACE = 78,
  KC_RGUI = 79, KC_RALT = 80, KC_RCTL = 81, KC_LEFT = 82, LEFT = 82, KC_DOWN = 83, DOWN = 83, KC_RGHT = 84, RIGHT = 84,
}

-- Dynamic modifier map (rebuilt live from `keyboard/keymap` so remapped Cmd/Ctrl/Opt/Fn work!)
local MOD_KEYS = {
  [60] = "shift", [71] = "shift",
  [74] = "cmd",   [75] = "alt",
  [76] = "ctrl",  [77] = "ctrl",
  [79] = "fn",    [80] = "alt", [81] = "ctrl",
}

-- Canonical keyCode aliases (maps both QMK short names & Flux long names to canonical key)
local KEYCODE_ALIAS = {
  KC_ESCAPE = "KC_ESC", KC_BACKSPACE = "KC_BSPC", KC_ENTER = "KC_ENT", KC_RETURN = "KC_ENT",
  KC_SPACE = "KC_SPC", KC_MINUS = "KC_MINS", KC_EQUAL = "KC_EQL", KC_LBRACKET = "KC_LBRC",
  KC_RBRACKET = "KC_RBRC", KC_BSLASH = "KC_BSLS", KC_SCOLON = "KC_SCLN", KC_QUOTE = "KC_QUOT",
  KC_GRAVE = "KC_GRV", KC_COMMA = "KC_COMM", KC_SLASH = "KC_SLSH", KC_DELETE = "KC_DEL",
  KC_INSERT = "KC_INS", KC_PSCREEN = "KC_PSCR", KC_PGUP = "KC_PGUP", KC_PAGE_UP = "KC_PGUP",
  KC_PGDOWN = "KC_PGDN", KC_PAGE_DOWN = "KC_PGDN", KC_RIGHT = "KC_RGHT",
  KC_LEFT_SHIFT = "KC_LSFT", KC_RIGHT_SHIFT = "KC_RSFT",
  KC_LEFT_CTRL = "KC_LCTL", KC_RIGHT_CTRL = "KC_RCTL",
  KC_LEFT_ALT = "KC_LALT", KC_RIGHT_ALT = "KC_RALT",
  KC_LEFT_GUI = "KC_LGUI", KC_RIGHT_GUI = "KC_RGUI",
}

local function mod_kind_for_action(act)
  if type(act) ~= "table" then return nil end
  if tonumber(act.type) == 2 or act.isMomentary == true or act.layerControlType == "momentaryLayer" then
    return "fn"
  end
  local kc = string.upper(tostring(act.keyCode or ""))
  kc = KEYCODE_ALIAS[kc] or kc
  if kc == "KC_LSFT" or kc == "KC_RSFT" then return "shift" end
  if kc == "KC_LCTL" or kc == "KC_RCTL" then return "ctrl" end
  if kc == "KC_LALT" or kc == "KC_RALT" then return "alt" end
  if kc == "KC_LGUI" or kc == "KC_RGUI" then return "cmd" end
  return nil
end

local function apply_hardware_keymap(km)
  if type(km) ~= "table" or type(km.layers) ~= "table" or #km.layers == 0 then return end
  local l0 = km.layers[1]
  local acts = l0 and l0.keyActions
  if type(acts) ~= "table" or #acts == 0 then return end
  local next_mods = {}
  for idx, act in ipairs(acts) do
    local ki = idx - 1
    local mk = mod_kind_for_action(act)
    if mk then
      next_mods[ki] = mk
    end
    local kc = string.upper(tostring(act.keyCode or ""))
    if #kc > 0 then
      KEYCODE_TO_INDEX[kc] = ki
      local canon = KEYCODE_ALIAS[kc]
      if canon then KEYCODE_TO_INDEX[canon] = ki end
    end
  end
  if next(next_mods) ~= nil then
    MOD_KEYS = next_mods
  end
end

local function publish_layout_regions()
  if not geom then return end
  if geom.slots then
    bus.publish("layout/regions/slots", { owner = "keys-renderer", slots = geom.slots }, true)
  end
  if geom.key_area_rect then
    bus.publish("layout/regions/keys", {
      owner = "keys-renderer",
      bounds = geom.key_area_rect,
      key_count = geom.keys and #geom.keys or 0,
    }, true)
  end
end

function on_init()
  geom = keyboard.layout()
  local km = bus.get("keyboard/keymap")
  if type(km) == "table" then
    apply_hardware_keymap(km)
  end
  -- Hydrate any already-retained providers immediately
  local existing = bus.list("keys/providers/#")
  if type(existing) == "table" then
    for _, msg in ipairs(existing) do
      if type(msg.payload) == "table" then
        providers[msg.topic] = msg.payload
      end
    end
  end
  publish_layout_regions()
end

local function current_mod_mask()
  local m = 0
  if held.ctrl then m = m + 1 end
  if held.shift then m = m + 2 end
  if held.alt then m = m + 4 end
  if held.cmd then m = m + 8 end
  return m
end

local function matches_csv(haystack, csv)
  if type(csv) ~= "string" then return true end
  local trimmed = csv:match("^%s*(.-)%s*$")
  if trimmed == "" then return true end
  local hay = string.lower(tostring(haystack or ""))
  for token in string.gmatch(trimmed, "[^,]+") do
    local needle = string.lower(token:match("^%s*(.-)%s*$") or "")
    if needle ~= "" and string.find(hay, needle, 1, true) then
      return true
    end
  end
  return false
end

local function when_matches(when)
  if type(when) ~= "table" then return true end
  if when.app and #tostring(when.app) > 0 then
    local app_s = tostring(active_app.process_name or "") .. " " .. tostring(active_app.app_id or "") .. " " .. tostring(active_app.window_title or "")
    if not matches_csv(app_s, tostring(when.app)) then return false end
  end
  if when.profile and #tostring(when.profile) > 0 then
    local prof_s = tostring(active_profile.name or "") .. " " .. tostring(active_profile.id or "") .. " " .. tostring(active_app.profile_name or "")
    if not matches_csv(prof_s, tostring(when.profile)) then return false end
  end
  if when.layer ~= nil and tonumber(when.layer) ~= active_layer then
    return false
  end
  if when.fn ~= nil and (when.fn == true) ~= held.fn then
    return false
  end
  return true
end

local function resolve_key_idx(k)
  if type(k) == "number" then return math.floor(k) end
  if type(k) == "string" then
    local n = tonumber(k)
    if n then return math.floor(n) end
    local up = string.upper(k)
    return KEYCODE_TO_INDEX[up] or KEYCODE_TO_INDEX[KEYCODE_ALIAS[up] or ""]
  end
  return nil
end

local function parse_hex(hex, fallback)
  if type(hex) ~= "string" then return fallback end
  local s = hex:gsub("^#", "")
  if #s == 6 then
    local r = tonumber(s:sub(1, 2), 16)
    local g = tonumber(s:sub(3, 4), 16)
    local b = tonumber(s:sub(5, 6), 16)
    if r and g and b then return { r / 255.0, g / 255.0, b / 255.0, 1.0 } end
  end
  return fallback
end

function on_event(topic, payload)
  if string.sub(topic, 1, 15) == "keys/providers/" then
    -- Erlang/MQTT LWT tombstone (`nil` / null payload) removes the dead/disabled provider cleanly!
    if type(payload) == "table" then
      providers[topic] = payload
    else
      providers[topic] = nil
    end
  elseif topic == "input/keys/event" and type(payload) == "table" then
    local ki = payload.key_index
    if ki ~= nil then
      if payload.kind == "press" then
        held_by_idx[ki] = true
      elseif payload.kind == "release" then
        held_by_idx[ki] = nil
      end
      held.ctrl, held.shift, held.alt, held.cmd, held.fn = false, false, false, false, false
      for hki, _ in pairs(held_by_idx) do
        local mk = MOD_KEYS[hki]
        if mk then held[mk] = true end
      end
    end
  elseif topic == "keyboard/state" and type(payload) == "table" then
    caps_lock = payload.caps_lock == true
    local bf = tonumber(payload.active_layer_bitfield) or 1
    if bf >= 2 then
      -- Highest active layer bit > 0
      local lyr = 0
      local v = bf
      while v >= 2 do v = math.floor(v / 2); lyr = lyr + 1 end
      active_layer = lyr
    else
      active_layer = 0
    end
  elseif topic == "keyboard/geometry" and type(payload) == "table" then
    geom = payload
    publish_layout_regions()
  elseif topic == "keyboard/keymap" and type(payload) == "table" then
    apply_hardware_keymap(payload)
  elseif topic == "host/app/active" and type(payload) == "table" then
    active_app = payload
  elseif topic == "host/profile/active" and type(payload) == "table" then
    active_profile = payload
  end
end

function on_update(dt)
  anim_t = anim_t + dt
end

-- Crisp vector icon library shared by all shortcut packs, keymap media keys, and game decks!
local function draw_icon(ctx, icon, cx, cy, col, sc)
  local s = sc or 1.0
  local b = { math.min(1, col[1] * 1.18), math.min(1, col[2] * 1.18), math.min(1, col[3] * 1.18), col[4] }
  if icon == "copy" then
    ctx:round_rect(cx - 12 * s, cy - 12 * s, 16 * s, 19 * s, 3 * s, { 0, 0, 0, 0 }, 2.2 * s, col)
    ctx:round_rect(cx - 4 * s, cy - 5 * s, 16 * s, 19 * s, 3 * s, { 0.04, 0.06, 0.10, 0.72 }, 2.2 * s, b)
  elseif icon == "paste" then
    ctx:round_rect(cx - 11 * s, cy - 10 * s, 22 * s, 24 * s, 3.5 * s, { 0, 0, 0, 0 }, 2.2 * s, b)
    ctx:round_rect(cx - 6 * s, cy - 14 * s, 12 * s, 6 * s, 2 * s, b, 0, { 0, 0, 0, 0 })
    ctx:line(cx - 6 * s, cy + 1 * s, cx + 6 * s, cy + 1 * s, 2.0 * s, col)
    ctx:line(cx - 6 * s, cy + 7 * s, cx + 4 * s, cy + 7 * s, 2.0 * s, col)
  elseif icon == "cut" then
    ctx:circle(cx - 8 * s, cy + 9 * s, 5 * s, { 0, 0, 0, 0 }, 2.2 * s, b)
    ctx:circle(cx + 8 * s, cy + 9 * s, 5 * s, { 0, 0, 0, 0 }, 2.2 * s, b)
    ctx:line(cx - 5 * s, cy + 5 * s, cx + 9 * s, cy - 13 * s, 2.4 * s, b)
    ctx:line(cx + 5 * s, cy + 5 * s, cx - 9 * s, cy - 13 * s, 2.4 * s, b)
  elseif icon == "undo" then
    ctx:arc(cx + 2 * s, cy + 2 * s, 11 * s, -150, 75, 2.6 * s, b)
    ctx:line(cx - 11 * s, cy - 4 * s, cx - 11 * s, cy - 13 * s, 2.5 * s, b)
    ctx:line(cx - 11 * s, cy - 4 * s, cx - 2 * s, cy - 4 * s, 2.5 * s, b)
  elseif icon == "redo" then
    ctx:arc(cx - 2 * s, cy + 2 * s, 11 * s, -30, 195, 2.6 * s, b)
    ctx:line(cx + 11 * s, cy - 4 * s, cx + 11 * s, cy - 13 * s, 2.5 * s, b)
    ctx:line(cx + 11 * s, cy - 4 * s, cx + 2 * s, cy - 4 * s, 2.5 * s, b)
  elseif icon == "save" then
    ctx:round_rect(cx - 12 * s, cy - 12 * s, 24 * s, 24 * s, 3.5 * s, { 0, 0, 0, 0 }, 2.2 * s, b)
    ctx:round_rect(cx - 7 * s, cy - 12 * s, 12 * s, 9 * s, 1.5 * s, { 0, 0, 0, 0 }, 1.8 * s, col)
    ctx:round_rect(cx - 7 * s, cy + 2 * s, 14 * s, 10 * s, 1.5 * s, { 0, 0, 0, 0 }, 1.8 * s, col)
  elseif icon == "search" then
    ctx:circle(cx - 3 * s, cy - 3 * s, 9 * s, { 0, 0, 0, 0 }, 2.5 * s, b)
    ctx:line(cx + 4 * s, cy + 4 * s, cx + 13 * s, cy + 13 * s, 3.0 * s, b)
  elseif icon == "plus" then
    ctx:line(cx, cy - 11 * s, cx, cy + 11 * s, 2.8 * s, b)
    ctx:line(cx - 11 * s, cy, cx + 11 * s, cy, 2.8 * s, b)
  elseif icon == "close" then
    ctx:line(cx - 9 * s, cy - 9 * s, cx + 9 * s, cy + 9 * s, 2.8 * s, b)
    ctx:line(cx + 9 * s, cy - 9 * s, cx - 9 * s, cy + 9 * s, 2.8 * s, b)
  elseif icon == "refresh" then
    ctx:arc(cx, cy, 11 * s, -60, 240, 2.6 * s, b)
    ctx:line(cx + 9 * s, cy - 9 * s, cx + 12 * s, cy - 1 * s, 2.5 * s, b)
    ctx:line(cx + 9 * s, cy - 9 * s, cx + 2 * s, cy - 7 * s, 2.5 * s, b)
  elseif icon == "print" then
    ctx:round_rect(cx - 13 * s, cy - 4 * s, 26 * s, 12 * s, 3 * s, { 0, 0, 0, 0 }, 2.2 * s, b)
    ctx:round_rect(cx - 8 * s, cy - 12 * s, 16 * s, 8 * s, 1.5 * s, { 0, 0, 0, 0 }, 1.8 * s, col)
    ctx:round_rect(cx - 8 * s, cy + 4 * s, 16 * s, 9 * s, 1.5 * s, { 0.04, 0.06, 0.10, 0.7 }, 1.8 * s, b)
  elseif icon == "power" then
    ctx:arc(cx, cy + 1 * s, 10.5 * s, -55, 235, 2.6 * s, b)
    ctx:line(cx, cy - 13 * s, cx, cy - 1 * s, 2.8 * s, b)
  elseif icon == "select_all" then
    ctx:round_rect(cx - 11 * s, cy - 11 * s, 22 * s, 22 * s, 3 * s, { 0, 0, 0, 0 }, 2.0 * s, col)
    ctx:round_rect(cx - 6 * s, cy - 6 * s, 12 * s, 12 * s, 2 * s, b, 0, { 0, 0, 0, 0 })
  elseif icon == "flame" then
    ctx:ellipse(cx, cy + 4 * s, 12 * s, 15 * s, col)
    ctx:ellipse(cx, cy + 6 * s, 6 * s, 9 * s, { 1.0, 0.92, 0.55, col[4] })
    ctx:line(cx, cy - 14 * s, cx - 7 * s, cy + 2 * s, 5 * s, col)
    ctx:line(cx, cy - 14 * s, cx + 7 * s, cy + 2 * s, 5 * s, col)
  elseif icon == "snow" then
    ctx:line(cx, cy - 13 * s, cx, cy + 13 * s, 2.8 * s, b)
    ctx:line(cx - 12 * s, cy - 7 * s, cx + 12 * s, cy + 7 * s, 2.8 * s, b)
    ctx:line(cx - 12 * s, cy + 7 * s, cx + 12 * s, cy - 7 * s, 2.8 * s, b)
    ctx:circle(cx, cy, 4 * s, { 1, 1, 1, col[4] })
  elseif icon == "bolt" then
    ctx:line(cx + 3 * s, cy - 14 * s, cx - 6 * s, cy + 1 * s, 4.2 * s, b)
    ctx:line(cx - 6 * s, cy + 1 * s, cx + 4 * s, cy + 1 * s, 4.2 * s, b)
    ctx:line(cx + 4 * s, cy + 1 * s, cx - 4 * s, cy + 15 * s, 4.2 * s, b)
  elseif icon == "shield" then
    ctx:round_rect(cx - 11 * s, cy - 13 * s, 22 * s, 17 * s, 5 * s, col, 1.8 * s, b)
    ctx:line(cx - 10 * s, cy + 2 * s, cx, cy + 14 * s, 5 * s, col)
    ctx:line(cx + 10 * s, cy + 2 * s, cx, cy + 14 * s, 5 * s, col)
  elseif icon == "heart" then
    ctx:circle(cx - 6 * s, cy - 3 * s, 7.5 * s, col)
    ctx:circle(cx + 6 * s, cy - 3 * s, 7.5 * s, col)
    ctx:line(cx - 11 * s, cy + 1 * s, cx, cy + 13 * s, 7 * s, col)
    ctx:line(cx + 11 * s, cy + 1 * s, cx, cy + 13 * s, 7 * s, col)
  elseif icon == "sword" then
    ctx:line(cx - 11 * s, cy + 11 * s, cx + 12 * s, cy - 12 * s, 4.2 * s, b)
    ctx:line(cx - 11 * s, cy + 3 * s, cx - 3 * s, cy + 11 * s, 3.4 * s, col)
  elseif icon == "arrow" then
    ctx:line(cx - 12 * s, cy + 3 * s, cx + 11 * s, cy - 5 * s, 3.8 * s, b)
    ctx:line(cx + 11 * s, cy - 5 * s, cx + 2 * s, cy - 11 * s, 3.4 * s, b)
    ctx:line(cx + 11 * s, cy - 5 * s, cx + 5 * s, cy + 5 * s, 3.4 * s, b)
  elseif icon == "crown" then
    ctx:round_rect(cx - 13 * s, cy + 4 * s, 26 * s, 7 * s, 2.5 * s, col, 0, { 0, 0, 0, 0 })
    ctx:line(cx - 12 * s, cy + 4 * s, cx - 12 * s, cy - 8 * s, 3.8 * s, b)
    ctx:line(cx, cy + 4 * s, cx, cy - 12 * s, 4.2 * s, b)
    ctx:line(cx + 12 * s, cy + 4 * s, cx + 12 * s, cy - 8 * s, 3.8 * s, b)
  elseif icon == "mic" then
    ctx:round_rect(cx - 5 * s, cy - 13 * s, 10 * s, 15 * s, 5 * s, b, 0, { 0, 0, 0, 0 })
    ctx:arc(cx, cy - 1 * s, 9.5 * s, 0, 180, 2.6 * s, col)
    ctx:line(cx, cy + 8 * s, cx, cy + 14 * s, 2.6 * s, col)
  elseif icon == "cam" then
    ctx:round_rect(cx - 13 * s, cy - 8 * s, 26 * s, 17 * s, 4 * s, col, 1.8 * s, b)
    ctx:circle(cx, cy, 5.5 * s, { 0.04, 0.06, 0.10, col[4] }, 2.0 * s, { 1, 1, 1, col[4] })
  elseif icon == "star" then
    ctx:circle(cx, cy, 9 * s, col)
    ctx:line(cx, cy - 14 * s, cx, cy + 14 * s, 3.0 * s, b)
    ctx:line(cx - 14 * s, cy, cx + 14 * s, cy, 3.0 * s, b)
  elseif icon == "play" then
    ctx:line(cx - 7 * s, cy - 11 * s, cx + 10 * s, cy, 3.5 * s, b)
    ctx:line(cx - 7 * s, cy + 11 * s, cx + 10 * s, cy, 3.5 * s, b)
    ctx:line(cx - 7 * s, cy - 11 * s, cx - 7 * s, cy + 11 * s, 3.5 * s, b)
  elseif icon == "brightness" then
    ctx:circle(cx, cy, 6.5 * s, b)
    for a = 0, 315, 45 do
      local rad = math.rad(a)
      ctx:line(cx + math.cos(rad) * 10 * s, cy + math.sin(rad) * 10 * s, cx + math.cos(rad) * 14 * s, cy + math.sin(rad) * 14 * s, 2.2 * s, b)
    end
  elseif icon == "volume" then
    ctx:round_rect(cx - 11 * s, cy - 5 * s, 7 * s, 10 * s, 2 * s, b, 0, { 0, 0, 0, 0 })
    ctx:line(cx - 4 * s, cy - 5 * s, cx + 4 * s, cy - 11 * s, 3.0 * s, b)
    ctx:line(cx - 4 * s, cy + 5 * s, cx + 4 * s, cy + 11 * s, 3.0 * s, b)
    ctx:line(cx + 4 * s, cy - 11 * s, cx + 4 * s, cy + 11 * s, 3.0 * s, b)
    ctx:arc(cx + 5 * s, cy, 7 * s, -45, 45, 2.2 * s, col)
  else
    ctx:circle(cx, cy, 10 * s, { col[1], col[2], col[3], col[4] * 0.25 }, 3.0 * s, b)
  end
end

function on_render(ctx, stage)
  if stage ~= "under_key" or not geom or not geom.keys then return end
  local default_col = module.color("legend_color", "#f2f7ff")
  local mod_mask = current_mod_mask()

  -- Deterministic per-key claim reduction across ALL active providers in ONE atomic frame!
  local winners = {} -- key_index -> { priority, mode, rule }
  local badges = {}  -- key_index -> list of badge rules

  -- Deterministic sorted iteration across ALL active providers in ONE atomic frame!
  -- Fast path: check integer `rule.mods` BEFORE string `when_matches` so all ~727 shortcut
  -- rules reject in a single integer compare (~0.05ms instead of 30ms of Lua string matching!).
  local prov_topics = {}
  for t, _ in pairs(providers) do table.insert(prov_topics, t) end
  table.sort(prov_topics)
  for _, ptopic in ipairs(prov_topics) do
    local prov = providers[ptopic]
    if type(prov) == "table" and type(prov.rules) == "table" and when_matches(prov.when) then
      local base_prio = tonumber(prov.priority) or 10
      local base_mode = tostring(prov.mode or "replace")
      for _, rule in ipairs(prov.rules) do
        if type(rule) == "table" then
          local r_mods = rule.mods
          if r_mods == nil or r_mods == mod_mask then
            if rule.when == nil or when_matches(rule.when) then
              local ki = resolve_key_idx(rule.key)
              if ki ~= nil then
                local prio = rule.priority or base_prio
                local rmode = rule.mode or base_mode
                if rmode == "badge" then
                  badges[ki] = badges[ki] or {}
                  table.insert(badges[ki], rule)
                else
                  local cur = winners[ki]
                  if not cur or prio >= cur.priority then
                    winners[ki] = { priority = prio, mode = rmode, rule = rule }
                  end
                end
              end
            end
          end
        end
      end
    end
  end

  -- Render ONLY the winning face for each physical key at its parallax-shifted visual position!
  for _, key in ipairs(geom.keys) do
    local ki = key.key_index
    local w = winners[ki]
    local r = key.rect
    local dx = key.parallax_dx or 0
    local dy = key.parallax_dy or 0
    local vx = (key.visual_x and key.visual_x > 0) and key.visual_x or (key.center_x + dx)
    local vy = (key.visual_y and key.visual_y > 0) and key.visual_y or (key.center_y + dy)
    local rx, ry = r.x + dx, r.y + dy

    if w and w.mode ~= "hide" then
      local rule = w.rule
      local col = parse_hex(rule.color, default_col)
      local on_cd = (tonumber(rule.cd) or 0) > 0 and ((tonumber(rule.max_charges) or 0) == 0 or (tonumber(rule.charges) or 0) == 0)
      local ult_ready = (rule.ult_pct ~= nil and tonumber(rule.ult_pct) >= 100)
      local pulse = ult_ready and (0.55 + 0.45 * math.sin(anim_t * 7.5)) or 0.0

      -- Optional subtle translucent tile frame (ONLY when a module explicitly asks for tile_bg = true!)
      if rule.tile_bg then
        local border_a = on_cd and 0.35 or (0.75 + pulse * 0.25)
        ctx:round_rect(
          rx + 3, ry + 3, r.w - 6, r.h - 6, 10,
          { 0.03 + col[1] * 0.08, 0.05 + col[2] * 0.08, 0.08 + col[3] * 0.08, 0.48 },
          ult_ready and 2.6 or 1.5,
          { col[1], col[2], col[3], border_a }
        )
      end

      local cfg = module.config()
      local legend_style = tostring(cfg.legend_style or "dual_shift")
      local bold_leg = cfg.bold_legends ~= false

      -- Helper: crisp shadowed & subtly bolded text so legends stay super readable over bright video!
      local function draw_crisp_text(tx, ty, str, sz, c, bold)
        local a = c[4] or 1.0
        ctx:text(tx + 1.4, ty + 1.6, str, sz, { 0, 0, 0, math.min(0.86, a * 0.85) }, "center")
        ctx:text(tx, ty, str, sz, c, "center")
        if bold then
          ctx:text(tx + 0.75, ty, str, sz, { c[1], c[2], c[3], a * 0.78 }, "center")
        end
      end

      -- Resolve dynamic label (Dual-Shift emphasis OR classic Swap!)
      local raw_label = tostring(rule.label or "")
      local has_dual = (rule.shifted ~= nil and tostring(rule.shifted) ~= "" and legend_style == "dual_shift")
      local label = raw_label
      if not has_dual then
        if rule.shifted and held.shift then
          label = tostring(rule.shifted)
        elseif rule.shift_case then
          local upper = (held.shift ~= caps_lock)
          label = upper and string.upper(label) or string.lower(label)
        end
      end

      local has_img = type(rule.image) == "string" and #rule.image > 0
      local has_icon = type(rule.icon) == "string" and #rule.icon > 0
      if has_img then
        -- Real PNG/image icon right inside the keycap (unlocks Stream Deck buttons & custom icons!)
        local pad = 10
        local isz = math.min(r.w - pad * 2, r.h - pad * 2)
        local ix = vx - isz * 0.5
        local iy = (#label > 0) and (ry + 6) or (vy - isz * 0.5)
        if #label > 0 then isz = math.max(28, isz - 16); ix = vx - isz * 0.5 end
        ctx:image(ix, iy, isz, isz, rule.image, { 1, 1, 1, on_cd and 0.4 or 1.0 })
        if #label > 0 then
          local lsz = (#label <= 7) and 13 or 11
          draw_crisp_text(vx, ry + r.h - 20, label, lsz, { col[1], col[2], col[3], on_cd and 0.50 or 0.96 }, bold_leg)
        end
      elseif has_icon then
        local icon_y = (#label > 0) and (vy - 11) or vy
        local icon_a = on_cd and 0.32 or 0.96
        draw_icon(ctx, rule.icon, vx, icon_y, { col[1], col[2], col[3], icon_a }, 1.0)
        if #label > 0 then
          local lsz = (#label <= 6) and 14 or 12
          draw_crisp_text(vx, ry + r.h - 23, label, lsz, { col[1], col[2], col[3], on_cd and 0.50 or 0.95 }, bold_leg)
        end
      elseif has_dual then
        -- Dual-Legend Shift Highlight mode (`!` above `1`, `@` above `2`, etc.):
        -- Active glyph snaps bright & bold while the inactive glyph sits softly dimmed!
        local top_s = tostring(rule.shifted)
        local bot_s = raw_label
        local shift_on = (held.shift == true)
        local top_a = shift_on and 0.98 or 0.42
        local bot_a = shift_on and 0.42 or 0.98
        local top_sz = shift_on and 24 or 17
        local bot_sz = shift_on and 17 or 25
        draw_crisp_text(vx, vy - 28, top_s, top_sz, { col[1], col[2], col[3], top_a }, shift_on and bold_leg)
        draw_crisp_text(vx, vy + 2,  bot_s, bot_sz, { col[1], col[2], col[3], bot_a }, (not shift_on) and bold_leg)
      elseif #label > 0 then
        local sp = (#label > 7) and string.find(label, " ") or nil
        if sp then
          local l1 = string.sub(label, 1, sp - 1)
          local l2 = string.sub(label, sp + 1)
          if #l1 > 9 then l1 = string.sub(l1, 1, 8) .. "." end
          if #l2 > 9 then l2 = string.sub(l2, 1, 8) .. "." end
          local lsz = 13
          local a = on_cd and 0.50 or 0.98
          draw_crisp_text(vx, vy - 15, l1, lsz, { col[1], col[2], col[3], a }, bold_leg)
          draw_crisp_text(vx, vy + 2,  l2, lsz, { col[1], col[2], col[3], a }, bold_leg)
        else
          local disp = (#label > 9) and (string.sub(label, 1, 8) .. ".") or label
          local lsz = (#disp <= 1) and 31 or ((#disp <= 2) and 27 or ((#disp <= 5) and 19 or 14))
          local ly = vy - lsz * 0.44
          draw_crisp_text(vx, ly, disp, lsz, { col[1], col[2], col[3], on_cd and 0.50 or 0.98 }, bold_leg)
        end
      end

      -- Sweeping cooldown curtain + countdown timer (for game decks / timers)
      local cd = tonumber(rule.cd) or 0
      local max_cd = tonumber(rule.max_cd) or 0
      if cd > 0 and max_cd > 0 then
        local frac = math.max(0.0, math.min(1.0, cd / max_cd))
        if on_cd then
          local ch = (r.h - 10) * frac
          ctx:round_rect(rx + 5, ry + r.h - 5 - ch, r.w - 10, ch, 6, { 0.01, 0.02, 0.05, 0.58 }, 0, { 0, 0, 0, 0 })
          local cd_s = (cd >= 10) and string.format("%d", math.ceil(cd)) or string.format("%.1f", cd)
          ctx:text(vx + 1, vy - 15 + 1, cd_s, 26, { 0, 0, 0, 0.85 }, "center")
          ctx:text(vx, vy - 15, cd_s, 26, { 1, 1, 1, 0.98 }, "center")
        else
          ctx:rect(rx + 10, ry + 6, (r.w - 20) * (1.0 - frac), 3.5, { col[1], col[2], col[3], 0.9 })
        end
      end

      -- Ultimate radial ring
      if rule.ult_pct ~= nil then
        local pct = math.floor(tonumber(rule.ult_pct) + 0.5)
        ctx:arc(vx, vy - 11, 21, -90, -90 + (pct / 100.0) * 360.0, 2.8, { col[1], col[2], col[3], 0.92 })
        if pct < 100 then
          ctx:text(rx + r.w - 8, ry + 6, string.format("%d%%", pct), 12, { 1.0, 0.94, 0.65, 0.95 }, "right")
        end
      end

      -- Charge pill or sub-label (e.g. ammo "12/30")
      if (tonumber(rule.max_charges) or 0) > 0 then
        local bx, by = rx + r.w - 24, ry + 6
        ctx:round_rect(bx, by, 18, 18, 6, { 0.04, 0.06, 0.11, 0.82 }, 1.2, col)
        ctx:text(bx + 9, by + 1, tostring(rule.charges or 0), 13, { 1, 1, 1, 0.98 }, "center")
      elseif type(rule.sub) == "string" and #rule.sub > 0 then
        ctx:text(rx + r.w - 8, ry + 6, rule.sub, 12, { col[1], col[2], col[3], 0.92 }, "right")
      elseif rule.toggled ~= nil then
        local led = rule.toggled and { 0.35, 1.0, 0.55, 1.0 } or { 0.45, 0.5, 0.58, 0.45 }
        ctx:circle(rx + r.w - 13, ry + 13, 4.2, led)
      end
    end

    -- Non-replacing corner badges (`mode = "badge"`, e.g. Caps Lock active dot)
    if badges[ki] then
      for _, br in ipairs(badges[ki]) do
        local bc = parse_hex(br.color, { 0.35, 0.92, 1.0, 1.0 })
        ctx:circle(rx + r.w - 13, ry + 13, 4.5, bc)
      end
    end
  end
end
