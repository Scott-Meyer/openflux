# Key Faces: rules, parts, conditions and signals

Every key has a small screen under it. `keys-renderer` decides what each screen shows. It does that from declarative **rules** that modules (and the user, through Key Rules) publish on `keys/providers/<id>`. This document describes the vocabulary those rules speak.

## Principles

- **Centered or inset by default.** Anything drawn at a keycap's edge makes parallax and viewing-angle offsets obvious. Every part is drawn around the key's visual center, or inset from the edge. Edge styles exist only as explicit opt-ins (`tile_bg`, `cd_style = "curtain"`, `progress_style = "edge"`, `shape = "edge"`).
- **Rules describe what you want, not how to draw it.** A module never draws on keys directly. It says "F2 glows gold while the bedroom is Dim", and the renderer owns layout, layering, transitions and performance.
- **One vocabulary for modules and people.** Key Rules (the user's own rules, edited in Studio or with `fluxctl`) uses the same rule shape as modules. Anything a module can show, a person can show.
- **Transitions are free.** Glow, outline and backdrop fade between states. Nothing pops unless it asks to.

## Rule shape

```lua
{
  -- Which keys (any one of these):
  key = "KC_F2" | 3,                  -- a physical key (ANSI position name or index)
  keys = { "KC_F1", "KC_F2", 5 },     -- several
  group = "fkeys",                    -- named group (see below)
  action = "mqtt-bridge.scene_on",    -- wherever that action is bound on the active layer

  mode = "replace" | "patch" | "badge" | "hide",   -- default from the provider
  priority = 80,                      -- default from the provider
  mods = 8,                           -- only while exactly these modifiers are held
  when = { ... },                     -- conditions (see below)

  -- Parts (all optional):
  label = "Dim", shifted = "!", icon = "brightness", image = "media://x.png", color = "#ffd166",
  hide = "legend" | "all",            -- patch shorthand: drop the main legend (label, shifted, icon, image) or everything
  glow = true | 0.6 | { strength, color, pulse },
  outline = true | { color, shape = "ring" | "window" | "edge", width, pulse },
  backdrop = "#1a2b3c" | { color, alpha, shape = "window" | "edge", pulse },
  corners = { tl = "2x", tr = { dot = true }, bl = { icon = "star" }, br = { text = "9", color = "#f00" } },
  sub = "21°", toggled = true,        -- shorthands for the top-right corner
  cd = 3.2, max_cd = 8, cd_style = "clock" | "curtain",
  charges = 2, max_charges = 3, progress_style = "ring" | "edge",
  ult_pct = 62,
  tile_bg = true,                     -- opt-in full-keycap frame
}
```

### Precedence: one fold

For each key, every matching rule is applied from lowest to highest precedence. Precedence is priority, then provider topic, then position in the provider.

- `replace` resets the face to exactly what it says.
- `patch` edits the face so far. Omitted fields inherit, `false` clears one, and a supplied field replaces that field. `corners` merge slot by slot. Setting `icon` clears an inherited `image` and vice versa. `hide = "legend"` drops label, shifted symbol, icon and image, and keeps corners, effects and cooldowns. `hide = "all"` blanks the key.
- `badge` puts a dot in one corner slot (`corner = "tl" | "tr" | "bl" | "br"`, default `tr`).
- `hide` blanks the key. A blank key also suppresses signals while it wins.

So a lower-priority patch never changes a higher-priority replacement, and a higher-priority patch always does.

Priority bands: keymap legends 10, shortcuts 30, module faces around 40–80, **binding markers 90** (layer-tap `L2`, mod-tap modifier icons, published by `keys-keymap` as their own patch), and **Key Rules 100+** (the user's rules always win; later rows win ties).

### Groups

`all`, `fkeys` (F1–F12), `numbers` (number row), `letters`, `home_row` (A–; ), `mods` (Shift/Ctrl/Alt/Cmd/Fn), `arrows`, `nav` (Ins/Home/PgUp/PgDn/End/Del), `row:N` (physical rows 0–5).

### Conditions (`when`)

All listed conditions must hold.

| Condition | Meaning |
|---|---|
| `app = "obs,zoom"` | foreground app matches any of these |
| `profile = "Gaming"` | active profile matches |
| `layer = 0` | top active layer |
| `fn = true` | Fn held |
| `held = true` | this key is held down |
| `held = "KC_LSFT"` | that key is held |
| `caps = true` | Caps Lock on |
| `host = true` | a computer is connected |
| `idle_ms = { gt = 60000 }` | keyboard idle |
| `time = "22:00-06:00"` | local wall-clock range |
| `bus = { topic, path, eq / ne / gt / lt / gte / lte / ["in"] / exists / contains }` | a retained bus value; `path` is a dot path into the JSON payload |

`bus` may also be a list (all must hold). The renderer subscribes to exactly the topics conditions mention, and unsubscribes when no rule mentions them any more (at most 32, and no wildcards), so values from the computer arrive on the keyboard automatically. A topic with no value is *unavailable*. Every operator then fails except `exists = false`.

## Signals: momentary looks

Some things are events, not states: a notification, a build failing, a timer ending. Anything can publish on `keys/signal` (not retained):

```json
{ "keys": ["KC_F5"], "set": { "glow": { "color": "#ff5c7c", "pulse": true } }, "duration_ms": 3000 }
```

Signals are effects only: `glow` and `outline`. The legend never changes, so the cached legend layer isn't redrawn. The newest signal on a key wins. Re-sending an `id` refreshes its deadline, and `{ "id": ..., "clear": true }` ends it. Durations are capped at 60 s and active signals at 48. When a signal ends, the key shows whatever its rules say *now*, fading back. Selectors are the same as for rules (`key`, `keys`, `group`, `action`). For something that is a state, like MQTT "pending" or "offline", use a rule condition, not a signal.

`fluxctl keys signal F5 --outline --pulse --color '#ff5c7c'` sends one.

## Explain

Publish `keys/inspect/request` `{ "key": "F2", "request_id": "x" }`. The renderer answers on `keys/inspect/result` with every rule that targets the key, in precedence order. Each step names the provider, rule, priority and mode, whether it matched, and why not if it didn't. The answer also includes the resolved face and any active signal. `fluxctl keys explain F2` prints it. Studio's Keycaps tab shows it in words.

## Key Rules (the user's rules)

The `keys-custom` module is now **Key Rules**. Its `rules` list holds the user's rules in friendly fields:

| Field | Meaning |
|---|---|
| `keys` | `F1-F4`, `Esc, a, 1, comma`, a group name, `row:N`, `#N` (physical index), or `action:<id>`; comma-separated |
| `layer` / `app` / `held` / `caps` / `time` / `idle_seconds` | conditions |
| `condition_topic`, `condition_path`, `condition_op`, `condition_value` | a `bus` condition |
| `hide` | `legend` or `all` |
| `label`, `icon`, `color` | the legend |
| `glow` (`soft`, `subtle`, `pulse`) + `glow_color` | effects |
| `outline` (`ring`, `window`, `edge`) + `outline_color` | effects |
| `backdrop` | effect color |
| `corner_tl` … `corner_br` | text, `dot`, `icon:<name>`, or `none` to clear |

You can edit them three ways, all on the same data:
- **Studio:** the Keycaps tab.
- **CLI:** `fluxctl keys rules list|add|remove|enable|disable|move`.
- **Raw config:** `fluxctl modules config keys-custom --set rules='[...]'`.

Per-key and per-action overrides from the Keycaps tab keep working, as patches at the bottom of the user band. That's a behavior change from replacing the face: binding markers now stay, and `bg`/`border` now draw as an inset backdrop and outline.

## Performance

- Legends and corners are a cached 2D layer. The renderer redraws it only when providers, held keys, modifiers, layer, app, conditions or signals change. Glow, outline pulses and fades are GPU passes every frame and don't invalidate the cache.
- Conditions are re-evaluated on the events that can change them: provider changes, bus values they watch, layer, modifiers, app, and held keys (only when some rule uses `held`). Time and idle conditions are re-checked once a second, only while some rule uses them.
