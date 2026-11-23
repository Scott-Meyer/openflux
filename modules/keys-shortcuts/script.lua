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

local function base_common_rules(accent)
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
  for _, c in ipairs(common) do
    table.insert(out, { key = c.key, mods = 8, priority = 28, label = c.label, icon = c.icon, color = accent })
    table.insert(out, { key = c.key, mods = 1, priority = 28, label = c.label, icon = c.icon, color = accent })
  end
  table.insert(out, { key = "KC_Z", mods = 10, priority = 28, label = "Redo", icon = "redo", color = accent })
  table.insert(out, { key = "KC_Z", mods = 3,  priority = 28, label = "Redo", icon = "redo", color = accent })
  return out
end

local function find_shortcuts_dir()
  local mdir = module.dir()
  local candidates = {
    "/usr/lib/openflux/display/shortcuts",
    "/home/root/openflux/shortcuts",
    mdir .. "/../../assets/shortcuts",
    mdir .. "/../assets/shortcuts",
  }
  local home = os.getenv("HOME")
  if home and #home > 0 then
    table.insert(candidates, home .. "/Applications/OpenFlux Studio.app/Contents/Resources/assets/shortcuts")
    table.insert(candidates, home .. "/.local/share/openflux/assets/shortcuts")
  end
  for _, d in ipairs(candidates) do
    local files = fs.list_dir(d)
    if type(files) == "table" and #files > 0 then
      return d, files
    end
  end
  return nil, {}
end

local EXTRA_APP_ALIASES = {
  ["macos-terminal.json"] = "flightdeck,iterm,terminal,ghostty,warp,kitty,alacritty",
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

local function load_all_packs(accent)
  local rules = base_common_rules(accent)
  local dir, files = find_shortcuts_dir()
  if not dir then return rules end

  local seen_pack_key = {} -- prevent duplicate (pack_file, key, mods) within the same pack

  for _, fname in ipairs(files) do
    if string.sub(fname, -5) == ".json" then
      local f = io.open(dir .. "/" .. fname, "r")
      if f then
        local content = f:read("*a")
        f:close()
        local ok, doc = pcall(json.decode, content)
        if ok and type(doc) == "table" then
          local rows = doc.shortcuts or doc.rows
          if type(rows) == "table" then
            local is_os_base = (fname == "macos.json" or fname == "windows.json")
            local app_tokens = {}
            if not is_os_base then
              if type(doc.apps) == "table" and type(doc.apps.macos) == "table" then
                for _, a in ipairs(doc.apps.macos) do
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
            elseif fname == "windows.json" then
              -- Skip windows.json base overrides on macOS so macos.json owns default Cmd shortcuts
              rows = {}
            end

            for _, r in ipairs(rows) do
              if type(r) == "table" and not r.macHasSecondKey then
                local kc = r.macKeyCode
                local mods = tonumber(r.macModifiers) or 0
                local lbl = tostring(r.label or "")
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

  return rules
end

function on_init()
  local accent = tostring(module.config().accent or "#5ce1ff")
  local rules = load_all_packs(accent)
  module.log("info", "loaded " .. tostring(#rules) .. " declarative shortcut rules across all app packs")
  bus.publish("keys/providers/shortcuts", {
    priority = 30,
    mode = "replace",
    rules = rules,
  }, true)
end
