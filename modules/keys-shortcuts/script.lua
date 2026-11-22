-- App & OS Shortcuts Provider (`keys-shortcuts`, priority 30)
-- Consumes v1's exact `host/app/shortcuts` (`shortcutOverlays`) stream—plus built-in macOS/Windows
-- standalone common shortcuts (Cmd+C/V/X/Z/S/F/A/P/Q/W/T/N/R) so Copy/Paste icons work even
-- standalone—and publishes declarative rules to `keys/providers/shortcuts` for 0ms flicker-free display!

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

local function default_common_rules(accent)
  -- Fallback OS shortcuts (Cmd = mod 8 on macOS, Ctrl = mod 1 on Win/Linux) so standalone works too!
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
    table.insert(out, { key = c.key, mods = 8, label = c.label, icon = c.icon, color = accent })
    table.insert(out, { key = c.key, mods = 1, label = c.label, icon = c.icon, color = accent })
  end
  -- Cmd+Shift+Z = Redo (mods = 10) and Ctrl+Shift+Z = Redo (mods = 3)
  table.insert(out, { key = "KC_Z", mods = 10, label = "Redo", icon = "redo", color = accent })
  table.insert(out, { key = "KC_Z", mods = 3,  label = "Redo", icon = "redo", color = accent })
  return out
end

local function publish_from_overlays(overlays)
  local accent = tostring(module.config().accent or "#5ce1ff")
  local rules = {}
  if type(overlays) == "table" and #overlays > 0 then
    for _, ov in ipairs(overlays) do
      if type(ov) == "table" and ov.keyCode and ov.mods and tonumber(ov.mods) > 0 then
        local lbl = tostring(ov.label or "")
        local ic = pick_icon(lbl, ov.icon)
        table.insert(rules, {
          key = tostring(ov.keyCode),
          mods = tonumber(ov.mods),
          label = lbl,
          icon = ic,
          color = accent,
        })
      end
    end
  end
  if #rules == 0 then
    rules = default_common_rules(accent)
  end
  bus.publish("keys/providers/shortcuts", {
    priority = 30,
    mode = "replace",
    rules = rules,
  }, true)
end

function on_init()
  local cur = bus.get("host/app/shortcuts")
  if type(cur) == "table" and type(cur.overlays) == "table" then
    publish_from_overlays(cur.overlays)
  else
    publish_from_overlays(nil)
  end
end

function on_event(topic, payload)
  if topic == "host/app/shortcuts" and type(payload) == "table" then
    publish_from_overlays(payload.overlays)
  end
end
