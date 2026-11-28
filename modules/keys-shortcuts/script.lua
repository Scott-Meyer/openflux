-- Declarative App & OS Shortcuts Provider (`keys-shortcuts`)
-- True v2 architecture: loads ALL shortcut packs (`assets/shortcuts/*.json`) ONCE at init and
-- publishes ONE declarative ruleset with `when = { app = "..." }` predicates directly to
-- `keys/providers/shortcuts`.
--
-- ZERO app-switch relay chain! `keys-renderer` subscribes directly to the live push stream
-- `host/app/active` (<5ms via NSWorkspace.didActivateApplicationNotification) and evaluates
-- `when.app` + held modifiers atomically in ONE frame with ZERO flash!

local function pick_icon(label, hint)
  local s = string.lower(tostring(hint or "") .. " " .. tostring(label or ""))
  if string.find(s, "copy") then return "copy" end
  if string.find(s, "paste") then return "paste" end
  if string.find(s, "cut") then return "cut" end
  if string.find(s, "redo") then return "redo" end
  if string.find(s, "undo") then return "undo" end
  if string.find(s, "save") then return "save" end
  if string.find(s, "find") or string.find(s, "search") then return "search" end
  if string.find(s, "print") then return "print" end
  if string.find(s, "quit") then return "power" end
  if string.find(s, "close") then return "close" end
  if string.find(s, "new") or string.find(s, "add") or string.find(s, "zoom in") then return "plus" end
  if string.find(s, "refresh") or string.find(s, "reload") then return "refresh" end
  if string.find(s, "select all") then return "select_all" end
  if string.find(s, "camera") or string.find(s, "screen") then return "cam" end
  if string.find(s, "mic") or string.find(s, "mute") then return "mic" end
  if string.find(s, "play") or string.find(s, "pause") then return "play" end
  return nil
end

local function base_common_rules(accent, is_macos)
  local common = {
    { key = "KC_X", label = "Cut",    icon = "cut" },
    { key = "KC_C", label = "Copy",   icon = "copy" },
    { key = "KC_V", label = "Paste",  icon = "paste" },
    { key = "KC_Z", label = "Undo",   icon = "undo" },
    { key = "KC_S", label = "Save",   icon = "save" },
    { key = "KC_F", label = "Find",   icon = "search" },
    { key = "KC_A", label = "All",    icon = "select_all" },
    { key = "KC_P", label = "Print",  icon = "print" },
    { key = "KC_Q", label = "Quit",   icon = "power" },
    { key = "KC_W", label = "Close",  icon = "close" },
    { key = "KC_T", label = "Tab",    icon = "plus" },
    { key = "KC_N", label = "New",    icon = "plus" },
    { key = "KC_R", label = "Reload", icon = "refresh" },
  }
  local out = {}
  local primary_mod = is_macos and 8 or 1   -- Cmd (8) on macOS, Ctrl (1) on Windows/Linux
  local redo_mod = is_macos and 10 or 3     -- Cmd+Shift (10) on macOS, Ctrl+Shift (3) on Win/Linux
  for _, c in ipairs(common) do
    table.insert(out, { key = c.key, mods = primary_mod, priority = 28, label = c.label, icon = c.icon, color = accent })
  end
  table.insert(out, { key = "KC_Z", mods = redo_mod, priority = 28, label = "Redo", icon = "redo", color = accent })
  return out
end

local function find_shortcuts_dir()
  local mdir = module.dir()
  local candidates = {
    mdir .. "/shortcuts",
    "/home/root/keyglass/shortcuts",
    "/usr/lib/keyglass/display/shortcuts",
  }
  local home = os.getenv("HOME")
  if home and #home > 0 then
    table.insert(candidates, home .. "/.local/share/keyglass/assets/shortcuts")
    table.insert(candidates, home .. "/Applications/Keyglass Studio.app/Contents/Resources/assets/shortcuts")
  end
  table.insert(candidates, mdir .. "/../../assets/shortcuts")
  table.insert(candidates, mdir .. "/../assets/shortcuts")
  for _, d in ipairs(candidates) do
    local files = fs.list_dir(d)
    if type(files) == "table" and #files > 0 then
      return d, files
    end
  end
  return nil, {}
end

local EXTRA_APP_ALIASES = {
  ["macos-terminal.json"] = "iterm,terminal,ghostty,warp,kitty,alacritty",
  ["vscode.json"] = "cursor,code,visual studio code",
  ["chrome.json"] = "chrome,google chrome,arc,brave",
  ["slack.json"] = "slack,com.tinyspeck.slackmacgap",
  ["discord.json"] = "discord,com.hnc.discord",
  ["finder.json"] = "finder,com.apple.finder",
  ["safari.json"] = "safari,com.apple.safari",
  ["firefox.json"] = "firefox,org.mozilla.firefox",
  ["figma.json"] = "figma,com.figma.desktop",
  ["zoom.json"] = "zoom,us.zoom.xos",
}

local KEY_TOKEN_TO_KC = {
  ESC = "KC_ESC", ESCAPE = "KC_ESC",
  ENTER = "KC_ENT", RETURN = "KC_ENT",
  TAB = "KC_TAB", SPACE = "KC_SPC",
  BACKSPACE = "KC_BSPC", BKSP = "KC_BSPC", DELETE = "KC_DEL", DEL = "KC_DEL",
  UP = "KC_UP", DOWN = "KC_DOWN", LEFT = "KC_LEFT", RIGHT = "KC_RGHT",
  HOME = "KC_HOME", END = "KC_END", PAGEUP = "KC_PGUP", PGUP = "KC_PGUP",
  PAGEDOWN = "KC_PGDN", PGDN = "KC_PGDN",
  ["-"] = "KC_MINS", MINUS = "KC_MINS", ["_"] = "KC_MINS",
  ["="] = "KC_EQL", PLUS = "KC_EQL", ["+"] = "KC_EQL",
  ["["] = "KC_LBRC", ["]"] = "KC_RBRC", ["\\"] = "KC_BSLS",
  [";"] = "KC_SCLN", ["'"] = "KC_QUOT", ["`"] = "KC_GRV", ["~"] = "KC_GRV",
  [","] = "KC_COMM", ["."] = "KC_DOT", ["/"] = "KC_SLSH", ["?"] = "KC_SLSH",
}

-- Parses a shortcut combo string like "Cmd+Shift+N", "Ctrl+Opt+C", "Cmd+," into (kc, mods)
-- Skips multi-key chords (containing space like "Cmd+K Cmd+S").
local function parse_combo(str)
  if type(str) ~= "string" or #str == 0 then return nil, 0 end
  -- Skip multi-stroke chords (e.g. "Cmd+K Cmd+C") or alternatives ("Cmd+1 / Cmd+2")
  if string.find(str, " / ", 1, true) then
    str = str:match("^(.-)%s*/") or str
  end
  if string.find(str, " ", 1, true) then return nil, 0 end

  local mods = 0
  local key_tok = nil
  -- Preserve literal trailing '+' (e.g. "Cmd++" -> Zoom In = KC_EQL)
  if string.sub(str, -2) == "++" then
    str = string.sub(str, 1, -2) .. "PLUS"
  end
  for part in string.gmatch(str, "[^+]+") do
    local up = string.upper(part:match("^%s*(.-)%s*$") or "")
    if up == "CTRL" or up == "CONTROL" then
      mods = mods | 1
    elseif up == "SHIFT" then
      mods = mods | 2
    elseif up == "ALT" or up == "OPT" or up == "OPTION" then
      mods = mods | 4
    elseif up == "CMD" or up == "COMMAND" or up == "META" or up == "SUPER" or up == "WIN" then
      mods = mods | 8
    elseif #up > 0 then
      key_tok = up
    end
  end
  if mods == 0 or not key_tok then return nil, 0 end
  if KEY_TOKEN_TO_KC[key_tok] then return KEY_TOKEN_TO_KC[key_tok], mods end
  if #key_tok == 1 then
    local b = string.byte(key_tok)
    if (b >= 65 and b <= 90) or (b >= 48 and b <= 57) then
      return "KC_" .. key_tok, mods
    end
  end
  if key_tok:match("^F%d+$") then
    return "KC_" .. key_tok, mods
  end
  return nil, 0
end

local function effective_is_macos()
  local ap = bus.get("profiles/active")
  if type(ap) == "table" then
    local ov = ap.module_overrides
    if type(ov) == "table" and type(ov["keys-keymap"]) == "table" then
      local v = string.lower(tostring(ov["keys-keymap"].os_layout or ""))
      if v == "windows" or v == "linux" then return false end
      if v == "macos" or v == "darwin" then return true end
    end
    if type(ap.tags) == "table" then
      for _, t in ipairs(ap.tags) do
        local s = string.lower(tostring(t))
        if s == "windows" or s == "linux" then return false end
        if s == "macos" or s == "darwin" then return true end
      end
    end
  end
  local hc = bus.get("host/connection")
  local plat = (type(hc) == "table" and type(hc.host_platform) == "string") and string.lower(hc.host_platform) or "macos"
  return (plat == "macos" or plat == "darwin" or plat == "")
end

local function load_all_packs(accent)
  local is_macos = effective_is_macos()
  local rules = base_common_rules(accent, is_macos)
  local dir, files = find_shortcuts_dir()
  if not dir then return rules end

  local seen_pack_key = {} -- prevent duplicate (pack_file, key, mods) within the same pack

    for _, fname in ipairs(files) do
      if string.sub(fname, -5) == ".json" then
        local content = fs.read_text(dir .. "/" .. fname)
        if type(content) == "string" and #content > 0 then
          local ok, doc = pcall(json.decode, content)
        if ok and type(doc) == "table" then
          local rows = doc.shortcuts or doc.rows
          if type(rows) == "table" then
            local is_os_base = (fname == "macos.json" or fname == "windows.json")
            local app_tokens = {}
            if not is_os_base then
              local app_list = type(doc.apps) == "table" and (is_macos and doc.apps.macos or doc.apps.windows) or nil
              if type(app_list) == "table" then
                for _, a in ipairs(app_list) do
                  if type(a) == "string" and #a > 0 then table.insert(app_tokens, a) end
                end
              end
              if EXTRA_APP_ALIASES[fname] then
                table.insert(app_tokens, EXTRA_APP_ALIASES[fname])
              end
            end
            local when_tbl = nil
            local prio = 30
            if not is_os_base and #app_tokens > 0 then
              when_tbl = { app = table.concat(app_tokens, ",") }
              prio = 35
            elseif (is_macos and fname == "windows.json") or ((not is_macos) and fname == "macos.json") then
              rows = {}
            end

            for _, r in ipairs(rows) do
              if type(r) == "table" then
                local kc, mods = nil, 0
                if is_macos then
                  if type(r.macKeyCode) == "string" and tonumber(r.macModifiers) and not r.macHasSecondKey then
                    kc = r.macKeyCode
                    mods = tonumber(r.macModifiers)
                  elseif type(r.mac) == "string" then
                    kc, mods = parse_combo(r.mac)
                  end
                else
                  if type(r.winKeyCode) == "string" and tonumber(r.winModifiers) and not r.winHasSecondKey then
                    kc = r.winKeyCode
                    mods = tonumber(r.winModifiers)
                  elseif type(r.win) == "string" then
                    kc, mods = parse_combo(r.win)
                  elseif type(r.macKeyCode) == "string" and tonumber(r.macModifiers) and not r.macHasSecondKey then
                    kc = r.macKeyCode
                    local m = tonumber(r.macModifiers)
                    -- Remap Cmd (8) -> Ctrl (1) as graceful Windows fallback when no explicit win binding exists
                    if (m & 8) ~= 0 then m = (m & ~8) | 1 end
                    mods = m
                  end
                end
                local lbl = tostring(r.label or r.name or "")
                if type(kc) == "string" and #kc > 0 and mods > 0 and #lbl > 0 then
                  local dedup = fname .. ":" .. kc .. ":" .. tostring(mods)
                  if not seen_pack_key[dedup] then
                    seen_pack_key[dedup] = true
                    table.insert(rules, {
                      key = kc,
                      mods = mods,
                      priority = prio,
                      when = when_tbl,
                      label = lbl,
                      icon = pick_icon(lbl, r.icon),
                      color = accent,
                    })
                  end
                end
              end
            end
          end
        end
      end
    end
  end

  -- Also load any user-customized shortcut packs edited in Studio's Shortcuts tab!
  local custom_paths = { "/home/root/keyglass/v2/custom_shortcuts.json" }
  local home = os.getenv("HOME")
  if home and #home > 0 then
    table.insert(custom_paths, home .. "/.local/share/keyglass/config/custom_shortcuts.json")
  end
  for _, cp in ipairs(custom_paths) do
    local raw = fs.read_text(cp)
    if type(raw) == "string" and #raw > 0 then
      local ok, cdoc = pcall(json.decode, raw)
      if ok and type(cdoc) == "table" and type(cdoc.packs) == "table" then
        for _, pack in ipairs(cdoc.packs) do
          if type(pack) == "table" and type(pack.rows) == "table" then
            local tokens = {}
            if type(pack.apps) == "table" and type(pack.apps.macos) == "table" then
              for _, a in ipairs(pack.apps.macos) do
                if type(a) == "string" and #a > 0 then table.insert(tokens, a) end
              end
            end
            local when_tbl = (#tokens > 0) and { app = table.concat(tokens, ",") } or nil
            for _, r in ipairs(pack.rows) do
              if type(r) == "table" and not r.hidden then
                local kc, mods = nil, 0
                if type(r.macKeyCode) == "string" and tonumber(r.macModifiers) and not r.macHasSecondKey then
                  kc = r.macKeyCode
                  mods = tonumber(r.macModifiers)
                elseif type(r.mac) == "string" then
                  kc, mods = parse_combo(r.mac)
                end
                local lbl = tostring(r.label or r.name or "")
                if type(kc) == "string" and #kc > 0 and mods > 0 and #lbl > 0 then
                  table.insert(rules, {
                    key = kc,
                    mods = mods,
                    priority = 38,
                    when = when_tbl,
                    label = lbl,
                    icon = pick_icon(lbl, r.icon),
                    color = accent,
                  })
                end
              end
            end
          end
        end
      end
      break
    end
  end

  return rules
end

local last_is_macos = nil

local function publish_shortcuts()
  local accent = tostring(module.config().accent or "#5ce1ff")
  last_is_macos = effective_is_macos()
  local rules = load_all_packs(accent)
  module.log("info", "loaded " .. tostring(#rules) .. " declarative shortcut rules (" .. (last_is_macos and "macOS" or "Windows/Linux") .. ")")
  bus.publish("keys/providers/shortcuts", {
    priority = 30,
    mode = "replace",
    rules = rules,
  }, true)
end

function on_init()
  publish_shortcuts()
end

function on_event(topic, _payload)
  if topic == "host/connection" or topic == "profiles/active" then
    local now_mac = effective_is_macos()
    if now_mac ~= last_is_macos then
      publish_shortcuts()
    end
  end
end
