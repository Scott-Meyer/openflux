-- Modular Keycap Face Renderer (`keys-renderer`)
-- Evaluates all retained `keys/providers/#` declarations (keymap, shortcuts, game decks, badges)
-- against live held modifiers, Caps Lock, Fn layer, foreground app, and active profile inside
-- ONE single atomic frame—zero 1-frame flicker, 100% transparent over video/ripples, and
-- automatically aligned to each key's optical parallax (`visual_x`, `visual_y`).

local geom = nil
local providers = {} -- topic -> provider table (nil tombstone removes it on Erlang/LWT purge!)
local held = { ctrl = false, shift = false, alt = false, cmd = false, fn = false }
local held_by_idx = {} -- physical key_index -> true while held (prevents dual-Shift/dual-Ctrl glitches!)
local qmk_state_active = false
local caps_lock = false
local active_layer = 0
local active_app = { app_id = "", process_name = "", window_title = "", profile_name = "" }
local active_profile = { index = 0, id = "", name = "" }
local anim_t = 0.0
local needs_redraw = true
local has_animated_rule = false
local parallax_refresh_frames = 0
local show_alignment_grid = false
local fx_targets = {}  -- key_index -> { glow?, outline?, backdrop? } from the last full redraw
local fx_levels = {}   -- "<key>:<part>" -> { level 0..1, spec }, eased toward its target every frame
local last_dt = 0.016
local key_boxes = {}   -- key_index -> { x, y, w, h } visual center + size (from the last redraw)
local blank_keys = {}  -- key_index -> true while a Blank rule wins (suppresses signals too)

-- Conditions & selectors state
local host_connected = true
local resolved = nil          -- keyboard/resolved_keys payload
local action_keycodes = {}    -- action_id -> numeric keycode (from system/actions)
local bus_values = {}         -- topic -> latest payload for topics conditions watch
local bus_watched = {}        -- topic -> true (runtime subscriptions owned by conditions)
local MAX_WATCHED = 32
local uses_held = false       -- some matching-candidate rule depends on held keys
local uses_clock = false      -- some rule depends on time of day or idle time (re-checked each second)
local idle_ms = 0
local clock_tick = 0

-- Signals: momentary effects (glow / outline) on keys, published on `keys/signal`.
local signals = {}            -- id -> { keyset, fx = { glow?, outline? }, until_t, seq }
local signal_seq = 0
local MAX_SIGNALS = 48
local MAX_SIGNAL_MS = 60000

-- Detail sits inset from the keycap edge: anything at the edge makes parallax and viewing-angle
-- offsets obvious. Corner slots are this fraction of the key size in from each edge.
local CORNER_INSET = 0.22
-- The cooldown clock and progress ring stay this fraction of the half-size in from the edge.
local RING_FRAC = 0.80

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
  local px = bus.get("display/parallax")
  if type(px) == "table" then
    show_alignment_grid = (px.showAlignmentGrid == true)
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

-- Named key groups (physical ANSI positions).
local GROUP_NAMES = {
  fkeys = { "KC_F1", "KC_F2", "KC_F3", "KC_F4", "KC_F5", "KC_F6", "KC_F7", "KC_F8", "KC_F9", "KC_F10", "KC_F11", "KC_F12" },
  numbers = { "KC_GRV", "KC_1", "KC_2", "KC_3", "KC_4", "KC_5", "KC_6", "KC_7", "KC_8", "KC_9", "KC_0", "KC_MINS", "KC_EQL" },
  home_row = { "KC_A", "KC_S", "KC_D", "KC_F", "KC_G", "KC_H", "KC_J", "KC_K", "KC_L", "KC_SCLN", "KC_QUOT" },
  arrows = { "KC_UP", "KC_LEFT", "KC_DOWN", "KC_RGHT" },
  nav = { "KC_INS", "KC_HOME", "KC_PGUP", "KC_PGDN", "KC_END", "KC_DEL" },
  mods = { "KC_LSFT", "KC_RSFT", "KC_LCTL", "KC_RCTL", "KC_LALT", "KC_RALT", "KC_LGUI", "KC_RGUI" },
}
local group_cache = {}

local function group_keys(name)
  if group_cache[name] then return group_cache[name] end
  local out = {}
  if name == "all" or name == "letters" or string.sub(name, 1, 4) == "row:" then
    local row = string.sub(name, 1, 4) == "row:" and tonumber(string.sub(name, 5)) or nil
    for _, k in ipairs((geom and geom.keys) or {}) do
      if name == "all" or (row ~= nil and k.row == row) then table.insert(out, k.key_index) end
    end
    if name == "letters" then
      for c = string.byte("A"), string.byte("Z") do
        local ki = resolve_key_idx("KC_" .. string.char(c))
        if ki then table.insert(out, ki) end
      end
    end
  else
    for _, kn in ipairs(GROUP_NAMES[name] or {}) do
      local ki = resolve_key_idx(kn)
      if ki then table.insert(out, ki) end
    end
  end
  if geom and geom.keys then group_cache[name] = out end
  return out
end

-- Every physical key a rule (or signal) targets: `key`, `keys`, `group`, or `action`
-- (wherever that action is bound on the active layer).
local function rule_targets(rule)
  if rule.group ~= nil then return group_keys(tostring(rule.group)) end
  if rule.action ~= nil then
    local kc = action_keycodes[tostring(rule.action)]
    local out = {}
    if kc and type(resolved) == "table" and type(resolved.keys) == "table" then
      for idx, e in ipairs(resolved.keys) do
        if type(e) == "table" and math.floor(tonumber(e.keycode) or -1) == kc then table.insert(out, idx - 1) end
      end
    end
    return out
  end
  if type(rule.keys) == "table" then
    local out = {}
    for _, k in ipairs(rule.keys) do
      local ki = resolve_key_idx(k)
      if ki ~= nil then table.insert(out, ki) end
    end
    return out
  end
  local ki = resolve_key_idx(rule.key)
  if ki ~= nil then return { ki } end
  return {}
end

-- `bus` condition: { topic, path?, eq | ne | ["in"] | exists | gt | lt | gte | lte | contains }.
-- A topic with no value (never published, cleared, or not mirrored yet) is *unavailable*:
-- the condition is false whatever the operator, except `exists = false`.
local function bus_value(topic, path)
  local v = bus_values[topic]
  if v == nil then return nil end
  if type(path) == "string" and #path > 0 then
    for part in string.gmatch(path, "[^%.]+") do
      if type(v) ~= "table" then return nil end
      local n = tonumber(part)
      v = (n and v[n + 1] ~= nil) and v[n + 1] or v[part]
      if v == nil then return nil end
    end
  end
  return v
end

local function same(a, b)
  if type(a) == "number" or type(b) == "number" then return tonumber(a) ~= nil and tonumber(a) == tonumber(b) end
  if type(a) == "boolean" or type(b) == "boolean" then return tostring(a) == tostring(b) end
  return tostring(a) == tostring(b)
end

local function bus_cond_holds(c, wanted_topics)
  if type(c) ~= "table" or type(c.topic) ~= "string" or #c.topic == 0 then return false end
  if wanted_topics then wanted_topics[c.topic] = true end
  local v = bus_value(c.topic, c.path)
  if c.exists ~= nil then return (v ~= nil) == (c.exists == true) end
  if v == nil then return false end
  if c.eq ~= nil and not same(v, c.eq) then return false end
  if c.ne ~= nil and same(v, c.ne) then return false end
  if type(c["in"]) == "table" then
    local hit = false
    for _, x in ipairs(c["in"]) do if same(v, x) then hit = true break end end
    if not hit then return false end
  end
  local nv = tonumber(v)
  if c.gt ~= nil and not (nv and nv > tonumber(c.gt)) then return false end
  if c.lt ~= nil and not (nv and nv < tonumber(c.lt)) then return false end
  if c.gte ~= nil and not (nv and nv >= tonumber(c.gte)) then return false end
  if c.lte ~= nil and not (nv and nv <= tonumber(c.lte)) then return false end
  if c.contains ~= nil and not string.find(tostring(v), tostring(c.contains), 1, true) then return false end
  return true
end

local bus_cond_holds_ref = bus_cond_holds

local wanted_topics_ref = nil  -- topics conditions referenced during the current resolve

local function when_matches(when, ki)
  if type(when) ~= "table" then return true end
  if when.held ~= nil then
    uses_held = true
    if type(when.held) == "boolean" then
      if ki == nil or (held_by_idx[ki] == true) ~= when.held then return false end
    else
      local hk = resolve_key_idx(when.held)
      if hk == nil or not held_by_idx[hk] then return false end
    end
  end
  if when.caps ~= nil and (when.caps == true) ~= caps_lock then return false end
  if when.time ~= nil then
    -- "HH:MM-HH:MM" in local time; a range may wrap past midnight ("22:00-06:00").
    uses_clock = true
    local h1, m1, h2, m2 = string.match(tostring(when.time), "^%s*(%d+):(%d+)%s*%-%s*(%d+):(%d+)%s*$")
    if not h1 then return false end
    local t = os.date("*t")
    local now = t.hour * 60 + t.min
    local a, b = tonumber(h1) * 60 + tonumber(m1), tonumber(h2) * 60 + tonumber(m2)
    local inside = (a <= b) and (now >= a and now < b) or (now >= a or now < b)
    if not inside then return false end
  end
  if when.idle_ms ~= nil then
    -- A number means "idle at least this long"; or { gt = ..., lt = ... }.
    uses_clock = true
    local c = when.idle_ms
    if type(c) == "number" then
      if idle_ms < c then return false end
    elseif type(c) == "table" then
      if c.gt ~= nil and not (idle_ms > tonumber(c.gt)) then return false end
      if c.lt ~= nil and not (idle_ms < tonumber(c.lt)) then return false end
    end
  end
  if when.host ~= nil and (when.host == true) ~= host_connected then return false end
  if when.bus ~= nil and bus_cond_holds_ref then
    local list = (type(when.bus) == "table" and when.bus.topic == nil) and when.bus or { when.bus }
    for _, c in ipairs(list) do
      if not bus_cond_holds_ref(c, wanted_topics_ref) then return false end
    end
  end
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

local handle_signal, handle_inspect -- defined after the face resolver

function on_event(topic, payload)
  if bus_watched[topic] then
    -- A topic some rule's `when.bus` condition watches (can also be one handled below).
    bus_values[topic] = payload
    needs_redraw = true
  end
  if topic == "keys/signal" then
    if type(payload) == "table" then handle_signal(payload) end
    return
  elseif topic == "keys/inspect/request" then
    if type(payload) == "table" then handle_inspect(payload) end
    return
  elseif topic == "keyboard/resolved_keys" then
    resolved = type(payload) == "table" and payload or nil
    needs_redraw = true
    return
  elseif topic == "system/actions" then
    action_keycodes = {}
    local slots = type(payload) == "table" and (payload.slots or payload) or {}
    for _, a in pairs(slots) do
      if type(a) == "table" and type(a.action_id) == "string" and tonumber(a.keycode) then
        action_keycodes[a.action_id] = math.floor(tonumber(a.keycode))
      end
    end
    needs_redraw = true
    return
  elseif topic == "host/connection" then
    local now = type(payload) == "table" and payload.connected ~= false
    if now ~= host_connected then host_connected = now; needs_redraw = true end
    return
  end
  if string.sub(topic, 1, 15) == "keys/providers/" then
    -- Erlang/MQTT LWT tombstone (`nil` / null payload) removes the dead/disabled provider cleanly!
    if type(payload) == "table" then
      providers[topic] = payload
    else
      providers[topic] = nil
    end
    needs_redraw = true
    elseif topic == "input/keys/event" and type(payload) == "table" then
    local ki = payload.key_index
    if ki ~= nil then
      if payload.kind == "press" then
        held_by_idx[ki] = true
      elseif payload.kind == "release" then
        held_by_idx[ki] = nil
      end
      if uses_held then needs_redraw = true end
      if payload.kind == "press" then
        if uses_clock and idle_ms > 0 then needs_redraw = true end
        idle_ms = 0
      end
      if not qmk_state_active then
        local prev_mask = current_mod_mask()
        local prev_fn = held.fn
        held.ctrl, held.shift, held.alt, held.cmd, held.fn = false, false, false, false, false
        for hki, _ in pairs(held_by_idx) do
          local mk = MOD_KEYS[hki]
          if mk then held[mk] = true end
        end
        if current_mod_mask() ~= prev_mask or held.fn ~= prev_fn then
          needs_redraw = true
        end
      end
    end
  elseif topic == "keyboard/state" and type(payload) == "table" then
    if tonumber(payload.idle_ms) then idle_ms = tonumber(payload.idle_ms) end
    local next_caps = (payload.caps_lock == true) or (payload.caps_word_active == true)
    local bf = tonumber(payload.active_layer_bitfield) or 1
    local def_bf = tonumber(payload.default_layer_bitfield) or 1
    local next_layer = 0
    if bf >= 2 then
      local v = bf
      while v >= 2 do v = math.floor(v / 2); next_layer = next_layer + 1 end
    end
    local prev_mask = current_mod_mask()
    local prev_fn = held.fn
    if payload.qmk_state_valid == true then
      qmk_state_active = true
      local em = tonumber(payload.effective_mods) or 0
      local om = tonumber(payload.oneshot_mods) or 0
      -- Combine QMK active + one-shot 8-bit HID modifier masks (bits 0/4 Ctrl, 1/5 Shift, 2/6 Alt, 3/7 GUI)
      local function bit_set(val, b) return math.floor(val / (2 ^ b)) % 2 == 1 end
      held.ctrl = bit_set(em, 0) or bit_set(em, 4) or bit_set(om, 0) or bit_set(om, 4)
      held.shift = bit_set(em, 1) or bit_set(em, 5) or bit_set(om, 1) or bit_set(om, 5)
      held.alt = bit_set(em, 2) or bit_set(em, 6) or bit_set(om, 2) or bit_set(om, 6)
      held.cmd = bit_set(em, 3) or bit_set(em, 7) or bit_set(om, 3) or bit_set(om, 7)
      held.fn = (bf ~= def_bf) and (next_layer > 0)
    end
    if next_caps ~= caps_lock or next_layer ~= active_layer or current_mod_mask() ~= prev_mask or held.fn ~= prev_fn then
      caps_lock = next_caps
      active_layer = next_layer
      needs_redraw = true
    end
  elseif topic == "keyboard/geometry" and type(payload) == "table" then
    -- Always read the authoritative local `keyboard.layout()` (which carries this device's live
    -- parallax-shifted `visual_x`, `visual_y`, `visual_rect`) so a remote host's unshifted
    -- startup geometry can never zero out on-device optical parallax!
    geom = keyboard.layout() or payload
    publish_layout_regions()
    needs_redraw = true
  elseif topic == "display/parallax" then
    if type(payload) == "table" and payload.showAlignmentGrid ~= nil then
      show_alignment_grid = (payload.showAlignmentGrid == true)
    end
    -- Refresh live optical parallax from shared `keyboard.layout()` immediately and over the
    -- next few frames (in case `apply_parallax_settings` runs right after event dispatch).
    geom = keyboard.layout()
    publish_layout_regions()
    parallax_refresh_frames = 4
    needs_redraw = true
  elseif topic == "keyboard/keymap" and type(payload) == "table" then
    apply_hardware_keymap(payload)
    needs_redraw = true
  elseif topic == "host/app/active" and type(payload) == "table" then
    if active_app.app_id ~= payload.app_id or active_app.process_name ~= payload.process_name then
      active_app = payload
      needs_redraw = true
    end
  elseif topic == "host/profile/active" and type(payload) == "table" then
    if active_profile.id ~= payload.id or active_profile.name ~= payload.name then
      active_profile = payload
      needs_redraw = true
    end
  end
end

function on_update(dt)
  anim_t = anim_t + dt
  last_dt = dt
  if uses_clock then
    clock_tick = clock_tick + dt
    if clock_tick >= 1.0 then
      clock_tick = 0
      idle_ms = idle_ms + 1000 -- advanced locally between keyboard/state reports
      needs_redraw = true
    end
  end
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
  -- Crisp Material Design / Apple keyboard modifier & navigation icons (matching Vialite)
  elseif icon == "kbd_cmd" then
    local w = 2.6 * s
    local r = 4.5 * s
    local d = 4.5 * s
    local e = d + r
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function cmd_pass(ox, oy, c)
      -- Continuous bars connecting all 4 outer cloverleaf loops tangentially
      ctx:line(cx + ox - e, cy + oy - d, cx + ox + e, cy + oy - d, w, c)
      ctx:line(cx + ox - e, cy + oy + d, cx + ox + e, cy + oy + d, w, c)
      ctx:line(cx + ox - d, cy + oy - e, cx + ox - d, cy + oy + e, w, c)
      ctx:line(cx + ox + d, cy + oy - e, cx + ox + d, cy + oy + e, w, c)
      -- 4 outer 270-degree cloverleaf loops (top-left, top-right, bottom-right, bottom-left)
      ctx:arc(cx + ox - e, cy + oy - e, r, 90, 360, w, c)
      ctx:arc(cx + ox + e, cy + oy - e, r, 180, 450, w, c)
      ctx:arc(cx + ox + e, cy + oy + e, r, 270, 540, w, c)
      ctx:arc(cx + ox - e, cy + oy + e, r, 0, 270, w, c)
    end
    cmd_pass(1.2, 1.4, sh)
    cmd_pass(0, 0, b)
  elseif icon == "kbd_win" then
    local g = 1.6 * s
    local q = 8.2 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function win_pass(ox, oy, c)
      ctx:round_rect(cx + ox - g - q, cy + oy - g - q, q, q, 1.4 * s, c, 0, { 0, 0, 0, 0 })
      ctx:round_rect(cx + ox + g,     cy + oy - g - q, q, q, 1.4 * s, c, 0, { 0, 0, 0, 0 })
      ctx:round_rect(cx + ox - g - q, cy + oy + g,     q, q, 1.4 * s, c, 0, { 0, 0, 0, 0 })
      ctx:round_rect(cx + ox + g,     cy + oy + g,     q, q, 1.4 * s, c, 0, { 0, 0, 0, 0 })
    end
    win_pass(1.2, 1.4, sh)
    win_pass(0, 0, b)
  elseif icon == "kbd_opt" then
    local w = 2.7 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function opt_pass(ox, oy, c)
      ctx:line(cx + ox - 13 * s, cy + oy - 8 * s, cx + ox - 5 * s, cy + oy - 8 * s, w, c)
      ctx:line(cx + ox - 5 * s,  cy + oy - 8 * s, cx + ox + 4 * s, cy + oy + 8 * s, w, c)
      ctx:line(cx + ox + 4 * s,  cy + oy + 8 * s, cx + ox + 13 * s, cy + oy + 8 * s, w, c)
      ctx:line(cx + ox + 2 * s,  cy + oy - 8 * s, cx + ox + 13 * s, cy + oy - 8 * s, w, c)
    end
    opt_pass(1.2, 1.4, sh)
    opt_pass(0, 0, b)
  elseif icon == "kbd_ctrl" then
    local w = 2.8 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function ctrl_pass(ox, oy, c)
      ctx:line(cx + ox - 11 * s, cy + oy + 5 * s, cx + ox, cy + oy - 7 * s, w, c)
      ctx:line(cx + ox, cy + oy - 7 * s, cx + ox + 11 * s, cy + oy + 5 * s, w, c)
    end
    ctrl_pass(1.2, 1.4, sh)
    ctrl_pass(0, 0, b)
  elseif icon == "kbd_shift" then
    local w = 2.5 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function sft_pass(ox, oy, c)
      ctx:line(cx + ox, cy + oy - 12 * s, cx + ox - 12 * s, cy + oy + 1 * s, w, c)
      ctx:line(cx + ox, cy + oy - 12 * s, cx + ox + 12 * s, cy + oy + 1 * s, w, c)
      ctx:line(cx + ox - 12 * s, cy + oy + 1 * s, cx + ox - 5 * s, cy + oy + 1 * s, w, c)
      ctx:line(cx + ox + 12 * s, cy + oy + 1 * s, cx + ox + 5 * s, cy + oy + 1 * s, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy + 1 * s, cx + ox - 5 * s, cy + oy + 11 * s, w, c)
      ctx:line(cx + ox + 5 * s, cy + oy + 1 * s, cx + ox + 5 * s, cy + oy + 11 * s, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy + 11 * s, cx + ox + 5 * s, cy + oy + 11 * s, w, c)
    end
    sft_pass(1.2, 1.4, sh)
    sft_pass(0, 0, b)
  elseif icon == "kbd_caps" then
    local w = 2.4 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function caps_pass(ox, oy, c)
      ctx:line(cx + ox, cy + oy - 13 * s, cx + ox - 11 * s, cy + oy - 2 * s, w, c)
      ctx:line(cx + ox, cy + oy - 13 * s, cx + ox + 11 * s, cy + oy - 2 * s, w, c)
      ctx:line(cx + ox - 11 * s, cy + oy - 2 * s, cx + ox - 5 * s, cy + oy - 2 * s, w, c)
      ctx:line(cx + ox + 11 * s, cy + oy - 2 * s, cx + ox + 5 * s, cy + oy - 2 * s, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy - 2 * s, cx + ox - 5 * s, cy + oy + 6 * s, w, c)
      ctx:line(cx + ox + 5 * s, cy + oy - 2 * s, cx + ox + 5 * s, cy + oy + 6 * s, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy + 6 * s, cx + ox + 5 * s, cy + oy + 6 * s, w, c)
      ctx:line(cx + ox - 6 * s, cy + oy + 12 * s, cx + ox + 6 * s, cy + oy + 12 * s, w + 0.5, c)
    end
    caps_pass(1.2, 1.4, sh)
    caps_pass(0, 0, b)
  elseif icon == "kbd_tab" then
    local w = 2.5 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function tab_pass(ox, oy, c)
      ctx:line(cx + ox - 13 * s, cy + oy, cx + ox + 6 * s, cy + oy, w, c)
      ctx:line(cx + ox + 1 * s, cy + oy - 5.5 * s, cx + ox + 6 * s, cy + oy, w, c)
      ctx:line(cx + ox + 1 * s, cy + oy + 5.5 * s, cx + ox + 6 * s, cy + oy, w, c)
      ctx:line(cx + ox + 11 * s, cy + oy - 8 * s, cx + ox + 11 * s, cy + oy + 8 * s, w, c)
    end
    tab_pass(1.2, 1.4, sh)
    tab_pass(0, 0, b)
  elseif icon == "kbd_bksp" then
    local w = 2.6 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function bk_pass(ox, oy, c)
      ctx:line(cx + ox + 13 * s, cy + oy, cx + ox - 12 * s, cy + oy, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy - 6.5 * s, cx + ox - 12 * s, cy + oy, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy + 6.5 * s, cx + ox - 12 * s, cy + oy, w, c)
    end
    bk_pass(1.2, 1.4, sh)
    bk_pass(0, 0, b)
  elseif icon == "kbd_enter" then
    local w = 2.6 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function ent_pass(ox, oy, c)
      ctx:line(cx + ox + 11 * s, cy + oy - 8 * s, cx + ox + 11 * s, cy + oy + 2 * s, w, c)
      ctx:line(cx + ox + 11 * s, cy + oy + 2 * s, cx + ox - 11 * s, cy + oy + 2 * s, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy - 4 * s, cx + ox - 11 * s, cy + oy + 2 * s, w, c)
      ctx:line(cx + ox - 5 * s, cy + oy + 8 * s, cx + ox - 11 * s, cy + oy + 2 * s, w, c)
    end
    ent_pass(1.2, 1.4, sh)
    ent_pass(0, 0, b)
  elseif icon == "kbd_space" then
    local w = 2.6 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function sp_pass(ox, oy, c)
      ctx:line(cx + ox - 14 * s, cy + oy, cx + ox - 14 * s, cy + oy + 6 * s, w, c)
      ctx:line(cx + ox - 14 * s, cy + oy + 6 * s, cx + ox + 14 * s, cy + oy + 6 * s, w, c)
      ctx:line(cx + ox + 14 * s, cy + oy + 6 * s, cx + ox + 14 * s, cy + oy, w, c)
    end
    sp_pass(1.2, 1.4, sh)
    sp_pass(0, 0, b)
  elseif icon == "kbd_home" then
    local w = 2.4 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function hm_pass(ox, oy, c)
      ctx:line(cx + ox - 9 * s, cy + oy - 10 * s, cx + ox + 9 * s, cy + oy - 10 * s, w, c)
      ctx:line(cx + ox, cy + oy + 10 * s, cx + ox, cy + oy - 5 * s, w, c)
      ctx:line(cx + ox - 6 * s, cy + oy + 1 * s, cx + ox, cy + oy - 5 * s, w, c)
      ctx:line(cx + ox + 6 * s, cy + oy + 1 * s, cx + ox, cy + oy - 5 * s, w, c)
    end
    hm_pass(1.2, 1.4, sh)
    hm_pass(0, 0, b)
  elseif icon == "kbd_end" then
    local w = 2.4 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function en_pass(ox, oy, c)
      ctx:line(cx + ox - 9 * s, cy + oy + 10 * s, cx + ox + 9 * s, cy + oy + 10 * s, w, c)
      ctx:line(cx + ox, cy + oy - 10 * s, cx + ox, cy + oy + 5 * s, w, c)
      ctx:line(cx + ox - 6 * s, cy + oy - 1 * s, cx + ox, cy + oy + 5 * s, w, c)
      ctx:line(cx + ox + 6 * s, cy + oy - 1 * s, cx + ox, cy + oy + 5 * s, w, c)
    end
    en_pass(1.2, 1.4, sh)
    en_pass(0, 0, b)
  elseif icon == "kbd_pgup" then
    local w = 2.5 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function pu_pass(ox, oy, c)
      ctx:line(cx + ox - 8 * s, cy + oy - 1 * s, cx + ox, cy + oy - 9 * s, w, c)
      ctx:line(cx + ox, cy + oy - 9 * s, cx + ox + 8 * s, cy + oy - 1 * s, w, c)
      ctx:line(cx + ox - 8 * s, cy + oy + 8 * s, cx + ox, cy + oy, w, c)
      ctx:line(cx + ox, cy + oy, cx + ox + 8 * s, cy + oy + 8 * s, w, c)
    end
    pu_pass(1.2, 1.4, sh)
    pu_pass(0, 0, b)
  elseif icon == "kbd_pgdn" then
    local w = 2.5 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function pd_pass(ox, oy, c)
      ctx:line(cx + ox - 8 * s, cy + oy - 8 * s, cx + ox, cy + oy, w, c)
      ctx:line(cx + ox, cy + oy, cx + ox + 8 * s, cy + oy - 8 * s, w, c)
      ctx:line(cx + ox - 8 * s, cy + oy + 1 * s, cx + ox, cy + oy + 9 * s, w, c)
      ctx:line(cx + ox, cy + oy + 9 * s, cx + ox + 8 * s, cy + oy + 1 * s, w, c)
    end
    pd_pass(1.2, 1.4, sh)
    pd_pass(0, 0, b)
  elseif icon == "kbd_up" or icon == "kbd_down" or icon == "kbd_left" or icon == "kbd_right" then
    local w = 2.6 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function arr_pass(ox, oy, c)
      if icon == "kbd_up" then
        ctx:line(cx + ox, cy + oy + 10 * s, cx + ox, cy + oy - 10 * s, w, c)
        ctx:line(cx + ox - 6 * s, cy + oy - 4 * s, cx + ox, cy + oy - 10 * s, w, c)
        ctx:line(cx + ox + 6 * s, cy + oy - 4 * s, cx + ox, cy + oy - 10 * s, w, c)
      elseif icon == "kbd_down" then
        ctx:line(cx + ox, cy + oy - 10 * s, cx + ox, cy + oy + 10 * s, w, c)
        ctx:line(cx + ox - 6 * s, cy + oy + 4 * s, cx + ox, cy + oy + 10 * s, w, c)
        ctx:line(cx + ox + 6 * s, cy + oy + 4 * s, cx + ox, cy + oy + 10 * s, w, c)
      elseif icon == "kbd_left" then
        ctx:line(cx + ox + 10 * s, cy + oy, cx + ox - 10 * s, cy + oy, w, c)
        ctx:line(cx + ox - 4 * s, cy + oy - 6 * s, cx + ox - 10 * s, cy + oy, w, c)
        ctx:line(cx + ox - 4 * s, cy + oy + 6 * s, cx + ox - 10 * s, cy + oy, w, c)
      else
        ctx:line(cx + ox - 10 * s, cy + oy, cx + ox + 10 * s, cy + oy, w, c)
        ctx:line(cx + ox + 4 * s, cy + oy - 6 * s, cx + ox + 10 * s, cy + oy, w, c)
        ctx:line(cx + ox + 4 * s, cy + oy + 6 * s, cx + ox + 10 * s, cy + oy, w, c)
      end
    end
    arr_pass(1.2, 1.4, sh)
    arr_pass(0, 0, b)
  elseif icon == "kbd_layer" then
    local w = 2.3 * s
    local sh = { 0, 0, 0, math.min(0.8, col[4] * 0.78) }
    local function lyr_pass(ox, oy, c)
      ctx:line(cx + ox, cy + oy - 11 * s, cx + ox + 11 * s, cy + oy - 4 * s, w, c)
      ctx:line(cx + ox + 11 * s, cy + oy - 4 * s, cx + ox, cy + oy + 2 * s, w, c)
      ctx:line(cx + ox, cy + oy + 2 * s, cx + ox - 11 * s, cy + oy - 4 * s, w, c)
      ctx:line(cx + ox - 11 * s, cy + oy - 4 * s, cx + ox, cy + oy - 11 * s, w, c)
      ctx:line(cx + ox - 11 * s, cy + oy + 2 * s, cx + ox, cy + oy + 8 * s, w, c)
      ctx:line(cx + ox, cy + oy + 8 * s, cx + ox + 11 * s, cy + oy + 2 * s, w, c)
    end
    lyr_pass(1.2, 1.4, sh)
    lyr_pass(0, 0, b)
  else
    ctx:circle(cx, cy, 10 * s, { col[1], col[2], col[3], col[4] * 0.25 }, 3.0 * s, b)
  end
end

-- ===== Effect parts (GPU, drawn every frame, fade in/out) =====
-- glow     = true | 0..1 | { strength, color, pulse }                 soft glow behind the symbol
-- outline  = true | { color, shape = "ring"|"window"|"edge", width, strength, pulse }
-- backdrop = "#rrggbb" | { color, alpha, shape = "window"|"edge", pulse }
local FX_PARTS = { "backdrop", "glow", "outline" }

local function fx_spec(part, v, col)
  if v == nil or v == false then return nil end
  local spec = { strength = 1.0, color = col, pulse = false }
  if part == "backdrop" then spec.strength = 0.55; spec.shape = "window" end
  if part == "outline" then spec.shape = "ring"; spec.width = 2.5 end
  if type(v) == "number" then
    spec.strength = math.max(0, math.min(1, v))
  elseif type(v) == "string" then
    spec.color = parse_hex(v, col)
  elseif type(v) == "table" then
    if v.strength ~= nil then spec.strength = math.max(0, math.min(1, tonumber(v.strength) or 1)) end
    if part == "backdrop" and v.alpha ~= nil then spec.strength = math.max(0, math.min(1, tonumber(v.alpha) or 0.55)) end
    if type(v.color) == "string" then spec.color = parse_hex(v.color, col) end
    if type(v.shape) == "string" then spec.shape = v.shape end
    if v.width ~= nil then spec.width = math.max(1, math.min(6, tonumber(v.width) or 2.5)) end
    spec.pulse = v.pulse == true
  end
  return spec
end

local function draw_fx_part(ctx, part, box, spec, lvl)
  local c = spec.color or { 1, 1, 1, 1 }
  local a = lvl * spec.strength
  local col = { c[1], c[2], c[3], math.min(1, a) }
  local inset = math.min(box.w, box.h) * 0.15 -- the keycap's clear window (same on wide keys)
  if part == "glow" then
    ctx:key_glow("glow", box.x, box.gy or box.y, 22, 22, 30.0, 0.0, a * 0.55, col)
  elseif part == "outline" then
    if spec.shape == "edge" then
      ctx:key_glow("ring", box.x, box.y, box.w - 8, box.h - 8, 1.0, spec.width, a, col)
    elseif spec.shape == "window" then
      ctx:key_glow("ring", box.x, box.y, box.w - inset * 2, box.h - inset * 2, 1.0, spec.width, a, col)
    else
      ctx:key_glow("ripple", box.x, box.y, 2.0, 2.0, math.min(box.w, box.h) * 0.5 * RING_FRAC, spec.width, a, col)
    end
  elseif part == "backdrop" then
    local w = spec.shape == "edge" and (box.w - 8) or (box.w - inset * 2)
    local h = spec.shape == "edge" and (box.h - 8) or (box.h - inset * 2)
    ctx:key_glow("solid", box.x, box.y, w, h, 1.0, 0.0, a, col)
  end
end

-- Active signals' effects on a key, newest first (only glow / outline).
local function signal_fx(ki)
  local best = nil
  for _, sg in pairs(signals) do
    if sg.until_t > anim_t and sg.keyset[ki] and (best == nil or sg.seq > best.seq) then best = sg end
  end
  return best
end

local function draw_fx(ctx)
  local k = math.min(1, last_dt * 6.0) -- ~0.4 s ease
  local pulse_f = 0.55 + 0.45 * math.sin(anim_t * 4.0)
  local seen = {}
  for ki, box in pairs(key_boxes) do
    local tg = (not blank_keys[ki]) and fx_targets[ki] or nil
    local sg = (not blank_keys[ki]) and signal_fx(ki) or nil
    for _, part in ipairs(FX_PARTS) do
      local spec = sg and sg.fx and sg.fx[part] or (tg and tg[part]) or nil
      local id = ki .. ":" .. part
      local st = fx_levels[id]
      if spec then
        seen[id] = true
        st = st or { level = 0 }
        local target = spec.pulse and pulse_f or 1.0
        st.level = st.level + (target - st.level) * k
        st.spec, st.box = spec, box
        fx_levels[id] = st
      end
    end
  end
  for id, st in pairs(fx_levels) do
    if not seen[id] then st.level = st.level * (1 - k) end
    if st.level < 0.004 then
      fx_levels[id] = nil
    else
      draw_fx_part(ctx, string.match(id, ":(%a+)$"), st.box, st.spec, st.level)
    end
  end
end

-- One inset corner slot. `item` is text, or { text | icon | dot = true | dot = "on"/"off", color }.
local function draw_corner(ctx, corner, item, rx, ry, r, col)
  if item == nil or item == false then return end
  if type(item) ~= "table" then item = { text = tostring(item) } end
  local c = type(item.color) == "string" and parse_hex(item.color, col) or col
  local right = corner == "tr" or corner == "br"
  local bottom = corner == "bl" or corner == "br"
  -- Inset by a fraction of the key's short side, so wide keys (Space, Shift) match the rest.
  local inset = math.min(r.w, r.h) * CORNER_INSET
  local cx = right and (rx + r.w - inset) or (rx + inset)
  local cy = bottom and (ry + r.h - inset) or (ry + inset)
  if item.dot ~= nil then
    local on = item.dot == true or item.dot == "on"
    local dc = on and (type(item.color) == "string" and c or { 0.35, 1.0, 0.55, 1.0 }) or { 0.45, 0.5, 0.58, 0.5 }
    ctx:circle(cx, cy, 6.0, { dc[1], dc[2], dc[3], dc[4] or 1.0 })
  elseif type(item.icon) == "string" and #item.icon > 0 then
    draw_icon(ctx, item.icon, cx, cy, { c[1], c[2], c[3], 0.95 }, 0.55)
  elseif item.text ~= nil and tostring(item.text) ~= "" then
    local t = tostring(item.text)
    local align = right and "right" or "left"
    -- Text hangs from the slot like a superscript (or sits on it, at the bottom).
    local ty = bottom and (cy - 13) or (cy - 7)
    ctx:text(cx, ty, t, 13, { c[1], c[2], c[3], 0.95 }, align)
  end
end

-- ===== Face resolution =====
-- Every rule that matches a key is folded from low to high precedence (priority, then provider
-- topic, then position): `replace` resets the face, `patch` edits named parts, `hide` blanks the
-- key, `badge` sets a dot in one corner slot. Omitted fields inherit, `false` clears.
local META = { key = true, keys = true, group = true, action = true, mode = true, priority = true,
  when = true, mods = true, corner = true, hide = true, sub = true, toggled = true }
local LEGEND_PARTS = { "label", "shifted", "icon", "image", "shift_case" }

local function normalized_corners(rule)
  local c = nil
  if type(rule.corners) == "table" then
    c = {}
    for slot, item in pairs(rule.corners) do c[slot] = item end
  elseif rule.corners == false then
    c = false
  end
  -- Shorthands land in the top-right slot (an explicit corners.tr wins).
  if rule.sub ~= nil or rule.toggled ~= nil then
    if c == nil or c == false then c = {} end
    if c.tr == nil then
      if rule.sub == false then
        c.tr = false
      elseif type(rule.sub) == "string" and #rule.sub > 0 then
        c.tr = { text = rule.sub }
      elseif rule.toggled ~= nil then
        c.tr = { dot = rule.toggled == true } -- false = a dimmed "off" dot
      end
    end
  end
  return c
end

local function apply_rule(face, rule, mode)
  if mode == "hide" or rule.hide == "all" then
    return { blank = true }
  end
  if mode == "badge" then
    face = face or {}
    local slot = (rule.corner == "tl" or rule.corner == "bl" or rule.corner == "br") and rule.corner or "tr"
    local corners = {}
    for k2, v2 in pairs(face.corners or {}) do corners[k2] = v2 end
    corners[slot] = { dot = true, color = type(rule.color) == "string" and rule.color or "#59ebff" }
    face.corners = corners
    return face
  end
  if mode == "replace" or face == nil or face.blank then face = {} end
  if rule.hide == "legend" or rule.hide == true then
    for _, f in ipairs(LEGEND_PARTS) do face[f] = nil end
  end
  for k2, v2 in pairs(rule) do
    if not META[k2] and k2 ~= "corners" then
      if v2 == false then
        face[k2] = nil
      else
        face[k2] = v2
        -- One symbol at a time: setting an icon clears an inherited image, and vice versa.
        if k2 == "icon" then face.image = nil elseif k2 == "image" then face.icon = nil end
      end
    end
  end
  local c = normalized_corners(rule)
  if c == false then
    face.corners = nil
  elseif c ~= nil then
    local merged = {}
    if mode ~= "replace" then
      for k2, v2 in pairs(face.corners or {}) do merged[k2] = v2 end
    end
    for slot, item in pairs(c) do
      if item == false then merged[slot] = nil else merged[slot] = item end
    end
    face.corners = merged
  end
  return face
end

-- Collect matching rules per key: key_index -> list of { prio, ord, mode, rule, topic }.
local function collect_matches(mod_mask, only_key, explain)
  local per_key = {}
  local wanted = {}
  wanted_topics_ref = wanted
  local prov_topics = {}
  for t, _ in pairs(providers) do table.insert(prov_topics, t) end
  table.sort(prov_topics)
  local ord = 0
  for pi, ptopic in ipairs(prov_topics) do
    local prov = providers[ptopic]
    if type(prov) == "table" and type(prov.rules) == "table" then
      local prov_ok = when_matches(prov.when)
      local base_prio = tonumber(prov.priority) or 10
      local base_mode = tostring(prov.mode or "replace")
      for ri, rule in ipairs(prov.rules) do
        if type(rule) == "table" then
          ord = ord + 1
          -- Fast path: integer `mods` check before anything else (~727 shortcut rules).
          local r_mods = rule.mods
          local mods_ok = (r_mods == nil or r_mods == mod_mask)
          if (mods_ok and prov_ok) or explain then
            for _, ki in ipairs(rule_targets(rule)) do
              if only_key == nil or only_key == ki then
                local ok = mods_ok and prov_ok and (rule.when == nil or when_matches(rule.when, ki))
                if ok or explain then
                  local lst = per_key[ki]
                  if not lst then lst = {}; per_key[ki] = lst end
                  table.insert(lst, {
                    prio = tonumber(rule.priority) or base_prio, ord = ord, mode = tostring(rule.mode or base_mode),
                    rule = rule, topic = ptopic, index = ri, matched = ok,
                    why = (not ok) and ((not prov_ok and "provider condition") or (not mods_ok and "modifiers") or "condition") or nil,
                  })
                end
              end
            end
          end
        end
      end
    end
  end
  wanted_topics_ref = nil
  return per_key, wanted
end

local function sort_matches(lst)
  table.sort(lst, function(a, b)
    if a.prio ~= b.prio then return a.prio < b.prio end
    return a.ord < b.ord
  end)
end

-- Topics that any rule's `when.bus` mentions, matched or not (so subscriptions don't churn as
-- modifiers or layers change).
local function wanted_bus_topics()
  local wanted = {}
  local function scan(when)
    if type(when) ~= "table" or when.bus == nil then return end
    local list = (type(when.bus) == "table" and when.bus.topic == nil) and when.bus or { when.bus }
    for _, c in ipairs(list) do
      if type(c) == "table" and type(c.topic) == "string" and #c.topic > 0 then wanted[c.topic] = true end
    end
  end
  for _, prov in pairs(providers) do
    if type(prov) == "table" then
      scan(prov.when)
      if type(prov.rules) == "table" then
        for _, rule in ipairs(prov.rules) do
          if type(rule) == "table" and rule.when ~= nil then scan(rule.when) end
        end
      end
    end
  end
  return wanted
end

local function resolve_faces(mod_mask)
  uses_held = false
  uses_clock = false
  local per_key = collect_matches(mod_mask)
  local wanted = wanted_bus_topics()
  local faces = {}
  for ki, lst in pairs(per_key) do
    if #lst > 1 then sort_matches(lst) end
    local face = nil
    for _, m in ipairs(lst) do face = apply_rule(face, m.rule, m.mode) end
    faces[ki] = face
  end
  -- Keep runtime subscriptions in step with the topics conditions mention (capped).
  local count = 0
  for t, _ in pairs(bus_watched) do
    if not wanted[t] then
      if bus.unsubscribe then bus.unsubscribe(t) end -- older runtimes keep it (harmless)
      bus_watched[t] = nil
      bus_values[t] = nil
    else
      count = count + 1
    end
  end
  for t, _ in pairs(wanted) do
    if not bus_watched[t] and count < MAX_WATCHED and not string.find(t, "[#+]") then
      bus_watched[t] = true
      bus_values[t] = bus.get(t)
      bus.subscribe(t)
      count = count + 1
      needs_redraw = true -- re-resolve with the value we just read
    end
  end
  return faces
end

handle_signal = function(p)
  local id = tostring(p.id or "")
  if p.clear == true then
    if #id > 0 then signals[id] = nil end
    return
  end
  local set = type(p.set) == "table" and p.set or { glow = true }
  local fx = {}
  local col = parse_hex(type(p.color) == "string" and p.color or nil, { 1.0, 0.82, 0.4, 1.0 })
  for _, part in ipairs({ "glow", "outline" }) do
    if set[part] ~= nil then fx[part] = fx_spec(part, set[part], col) end
  end
  if not next(fx) then return end
  local keyset = {}
  for _, ki in ipairs(rule_targets(p)) do keyset[ki] = true end
  if not next(keyset) then return end
  if #id == 0 then id = "anon:" .. tostring(anim_t) end
  local count = 0
  local oldest_id, oldest_seq = nil, math.huge
  for sid, sg in pairs(signals) do
    if sg.until_t <= anim_t then
      signals[sid] = nil
    else
      count = count + 1
      if sg.seq < oldest_seq then oldest_id, oldest_seq = sid, sg.seq end
    end
  end
  if not signals[id] and count >= MAX_SIGNALS and oldest_id then signals[oldest_id] = nil end
  local dur = math.max(50, math.min(MAX_SIGNAL_MS, tonumber(p.duration_ms) or 2000))
  signal_seq = (signal_seq or 0) + 1
  -- Re-sending an id refreshes its deadline and its look.
  signals[id] = { keyset = keyset, fx = fx, until_t = anim_t + dur / 1000.0, seq = signal_seq }
end

handle_inspect = function(p)
  local ki = resolve_key_idx(p.key)
  if ki == nil then return end
  local per_key = collect_matches(current_mod_mask(), ki, true)
  local lst = per_key[ki] or {}
  sort_matches(lst)
  local steps, face = {}, nil
  for _, m in ipairs(lst) do
    local step = { provider = m.topic, rule = m.index, priority = m.prio, mode = m.mode, matched = m.matched }
    if m.matched then
      face = apply_rule(face, m.rule, m.mode)
    else
      step.skipped = m.why
    end
    table.insert(steps, step)
  end
  local out_face = {}
  for k2, v2 in pairs(face or {}) do
    if type(v2) ~= "function" then out_face[k2] = v2 end
  end
  local sg = signal_fx(ki)
  bus.publish("keys/inspect/result", {
    key = ki,
    request_id = p.request_id,
    layer = active_layer,
    mods = current_mod_mask(),
    steps = steps, -- low to high precedence; later matched steps override earlier ones
    face = out_face,
    signal = sg and { until_ms = math.floor((sg.until_t - anim_t) * 1000) } or nil,
  }, false)
end

function on_render(ctx, stage)
  if stage ~= "under_key" or not geom or not geom.keys then return end
  draw_fx(ctx)
  if parallax_refresh_frames > 0 then
    geom = keyboard.layout()
    parallax_refresh_frames = parallax_refresh_frames - 1
    needs_redraw = true
  end
  -- Zero-allocation fast path: when no modifiers, providers, app, or animated cooldowns changed,
  -- tell the compositor to keep the cached keycap overlay as-is (saves ~273 FFI text draws/frame!).
  if not needs_redraw and not has_animated_rule then
    ctx:keep_layer()
    return
  end
  needs_redraw = false
  has_animated_rule = false
  fx_targets = {}
  blank_keys = {}
  local default_col = module.color("legend_color", "#f2f7ff")
  local mod_mask = current_mod_mask()
  local faces = resolve_faces(mod_mask)

  -- Render ONLY the winning face for each physical key at its parallax-shifted visual position!
  for _, key in ipairs(geom.keys) do
    local ki = key.key_index
    local face = faces[ki]
    local r = key.visual_rect or key.rect
    local dx = key.parallax_dx or 0
    local dy = key.parallax_dy or 0
    local vx = (key.visual_x and key.visual_x > 0) and key.visual_x or (key.center_x + dx)
    local vy = (key.visual_y and key.visual_y > 0) and key.visual_y or (key.center_y + dy)
    local rx = key.visual_rect and r.x or (r.x + dx)
    local ry = key.visual_rect and r.y or (r.y + dy)
    key_boxes[ki] = { x = vx, y = vy, w = r.w, h = r.h }
    if face and face.blank then blank_keys[ki] = true end

    -- Optical Parallax Alignment Grid: crisp cyan inner-window calibration box + center
    -- crosshair (`+`) inset to match the physical clear acrylic window inside each keycap
    -- (inset 15px from the outer 105.5px skirt so the box sits cleanly inside the clear cap).
    if show_alignment_grid then
      local inset = 15
      local gx = rx + inset
      local gy = ry + inset
      local gw = math.max(16, r.w - inset * 2)
      local gh = math.max(16, r.h - inset * 2)
      ctx:round_rect(
        gx, gy, gw, gh, 7,
        { 0.0, 0.85, 1.0, 0.08 },
        2.0,
        { 0.20, 0.92, 1.0, 0.92 }
      )
      ctx:line(vx - 7, vy, vx + 7, vy, 1.8, { 1.0, 0.82, 0.25, 0.95 })
      ctx:line(vx, vy - 7, vx, vy + 7, 1.8, { 1.0, 0.82, 0.25, 0.95 })
    end

    if face and not face.blank then
      local rule = face
      local col = parse_hex(rule.color, default_col)
      local on_cd = (tonumber(rule.cd) or 0) > 0 and ((tonumber(rule.max_charges) or 0) == 0 or (tonumber(rule.charges) or 0) == 0)
      local ult_ready = (rule.ult_pct ~= nil and tonumber(rule.ult_pct) >= 100)
      if on_cd or ult_ready then has_animated_rule = true end
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
      -- Effect parts: glow centers on the symbol (icon/image sit above a caption) or the legend.
      key_boxes[ki].gy = ((has_icon or has_img) and #label > 0) and (vy - 10) or vy
      local tg = nil
      for _, part in ipairs(FX_PARTS) do
        local spec = fx_spec(part, rule[part], col)
        if spec then tg = tg or {}; tg[part] = spec end
      end
      fx_targets[ki] = tg
      if has_img then
        -- Resolve portable `module://<id>/<rel>` and `media://<fname>` asset refs locally on
        -- BOTH Mac and keyboard so cross-device BrokerMirror messages never break image paths!
        local img_path = rule.image
        if string.sub(img_path, 1, 9) == "module://" then
          local rest = string.sub(img_path, 10)
          local mdir = module.dir():gsub("\\", "/")
          local parent = mdir:gsub("/[^/]+$", "")
          local cand = parent .. "/" .. rest
          if string.sub(mdir, 1, 5) ~= "/usr/" and string.sub(mdir, 1, 6) ~= "/home/" then
            -- On host (macOS, Linux, or Windows), optional modules may live in `local/` or `installed/`
            -- while `keys-renderer` is `bundled/`.
            local roots = {}
            local home = os.getenv("HOME")
            if home and #home > 0 then
              table.insert(roots, home .. "/.local/share/openflux/v2/modules")
            end
            local localappdata = os.getenv("LOCALAPPDATA")
            if localappdata and #localappdata > 0 then
              table.insert(roots, localappdata:gsub("\\", "/") .. "/openflux/v2/modules")
            end
            for _, root in ipairs(roots) do
              local local_p = root .. "/local/" .. rest
              local inst_p = root .. "/installed/" .. rest
              if fs.exists(local_p) then
                cand = local_p
                break
              elseif fs.exists(inst_p) then
                cand = inst_p
                break
              end
            end
          end
          img_path = cand
        elseif string.sub(img_path, 1, 8) == "media://" then
          local fname = string.sub(img_path, 9)
          local mdir = module.dir()
          if string.sub(mdir, 1, 5) == "/usr/" or string.sub(mdir, 1, 6) == "/home/" then
            img_path = "/home/root/openflux/media/" .. fname
          else
            local home = os.getenv("HOME")
            if home and #home > 0 then
              img_path = home .. "/.local/share/openflux/media/" .. fname
            end
          end
        end
        -- Real PNG/image icon right inside the keycap (unlocks Stream Deck buttons & custom icons!)
        local pad = 10
        local isz = math.min(r.w - pad * 2, r.h - pad * 2)
        local ix = vx - isz * 0.5
        local iy = (#label > 0) and (ry + 6) or (vy - isz * 0.5)
        if #label > 0 then isz = math.max(28, isz - 16); ix = vx - isz * 0.5 end
        ctx:image(ix, iy, isz, isz, img_path, { 1, 1, 1, on_cd and 0.4 or 1.0 })
        if #label > 0 then
          local lsz = (#label <= 7) and 13 or 11
          draw_crisp_text(vx, vy + 13, label, lsz, { col[1], col[2], col[3], on_cd and 0.50 or 0.96 }, bold_leg)
        end
      elseif has_icon then
        local icon_y = (#label > 0) and (vy - 10) or vy
        local icon_a = on_cd and 0.32 or 0.96
        draw_icon(ctx, rule.icon, vx, icon_y, { col[1], col[2], col[3], icon_a }, 1.0)
        if #label > 0 then
          local lsz = (#label <= 6) and 14 or 12
          -- Keep caption tucked closely below the symbol (centered inside the keycap sweet-spot
          -- so optical parallax never clips the caption off the bottom keycap edge!)
          draw_crisp_text(vx, vy + 11, label, lsz, { col[1], col[2], col[3], on_cd and 0.50 or 0.95 }, bold_leg)
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
          local ulen = (utf8 and utf8.len(label)) or #label
          local disp = (ulen > 9 and #label > 12) and (string.sub(label, 1, 8) .. ".") or label
          local dlen = (utf8 and utf8.len(disp)) or #disp
          local lsz = (dlen <= 1) and 31 or ((dlen <= 2) and 27 or ((dlen <= 5) and 19 or 14))
          local ly = vy - lsz * 0.44
          draw_crisp_text(vx, ly, disp, lsz, { col[1], col[2], col[3], on_cd and 0.50 or 0.98 }, bold_leg)
        end
      end

      -- Sweeping cooldown curtain + countdown timer (for game decks / timers)
      local cd = tonumber(rule.cd) or 0
      local max_cd = tonumber(rule.max_cd) or 0
      if cd > 0 and max_cd > 0 then
        local frac = math.max(0.0, math.min(1.0, cd / max_cd))
        local ring_r = math.min(r.w, r.h) * 0.5 * RING_FRAC
        if on_cd then
          if rule.cd_style == "curtain" then
            -- Edge-to-edge curtain (opt-in).
            local ch = (r.h - 10) * frac
            ctx:round_rect(rx + 5, ry + r.h - 5 - ch, r.w - 10, ch, 6, { 0.01, 0.02, 0.05, 0.58 }, 0, { 0, 0, 0, 0 })
          else
            -- Clock sweep centered on the key: the shaded sector is the time left, and it
            -- shrinks clockwise from 12 o'clock as the cooldown runs out.
            ctx:pie(vx, vy, ring_r, -90 + (1.0 - frac) * 360.0, 270, { 0.01, 0.02, 0.05, 0.55 })
          end
          local cd_s = (cd >= 10) and string.format("%d", math.ceil(cd)) or string.format("%.1f", cd)
          ctx:text(vx + 1, vy - 15 + 1, cd_s, 26, { 0, 0, 0, 0.85 }, "center")
          ctx:text(vx, vy - 15, cd_s, 26, { 1, 1, 1, 0.98 }, "center")
        elseif rule.progress_style == "edge" then
          -- Recharge bar along the top edge (opt-in).
          ctx:rect(rx + 10, ry + 6, (r.w - 20) * (1.0 - frac), 3.5, { col[1], col[2], col[3], 0.9 })
        else
          -- Recharge progress as a thin ring around the center.
          ctx:arc(vx, vy, ring_r, -90, -90 + (1.0 - frac) * 360.0, 2.5, { col[1], col[2], col[3], 0.85 })
        end
      end

      -- Ultimate radial ring
      if rule.ult_pct ~= nil then
        local pct = math.floor(tonumber(rule.ult_pct) + 0.5)
        ctx:arc(vx, vy - 11, 21, -90, -90 + (pct / 100.0) * 360.0, 2.8, { col[1], col[2], col[3], 0.92 })
        if pct < 100 then
          draw_corner(ctx, "tr", { text = string.format("%d%%", pct), color = "#fff0a6" }, rx, ry, r, col)
        end
      end

      -- Corner slots (inset from the edge). `rule.corners = { tl, tr, bl, br }`, each text or
      -- { text | icon | dot, color }. The older single-purpose fields fill the top-right slot.
      local corners = type(rule.corners) == "table" and rule.corners or {}
      local tr = corners.tr
      if tr == nil and (tonumber(rule.max_charges) or 0) > 0 then
        tr = { text = tostring(rule.charges or 0) }
      end
      if rule.ult_pct == nil or math.floor(tonumber(rule.ult_pct) + 0.5) >= 100 then
        draw_corner(ctx, "tr", tr, rx, ry, r, col)
      end
      draw_corner(ctx, "tl", corners.tl, rx, ry, r, col)
      draw_corner(ctx, "bl", corners.bl, rx, ry, r, col)
      draw_corner(ctx, "br", corners.br, rx, ry, r, col)
    end

  end
end
