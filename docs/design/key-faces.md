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
  key = "KC_F2" | 3,                  -- a physical key
  keys = { "KC_F1", "KC_F2", 5 },     -- several
  group = "fkeys",                    -- named group (see below)

  mode = "replace" | "patch" | "badge" | "hide",   -- default from the provider
  priority = 80,                      -- default from the provider
  mods = 8,                           -- only while exactly these modifiers are held
  when = { ... },                     -- conditions (see below)

  -- Parts (all optional):
  label = "Dim", shifted = "!", icon = "brightness", image = "media://x.png", color = "#ffd166",
  hide = "legend" | "all",            -- patch shorthand: drop the main legend (label, shifted, icon, image) or everything
  glow = true | 0.6 | { strength, color, pulse },
  outline = true | { color, shape = "ring" | "window" | "edge", width, pulse },
  backdrop = "#1a2b3c" | { color, alpha, shape = "disc" | "window" | "edge" },
  corners = { tl = "2x", tr = { dot = true }, bl = { icon = "star" }, br = { text = "9", color = "#f00" } },
  sub = "21°", toggled = true,        -- shorthands for the top-right corner
  cd = 3.2, max_cd = 8, cd_style = "clock" | "curtain",
  charges = 2, max_charges = 3, progress_style = "ring" | "edge",
  ult_pct = 62,
  tile_bg = true,                     -- opt-in full-keycap frame
}
```

### Modes

- `replace`: the highest-priority `replace` rule is the key's face.
- `patch`: edits whatever face won, in priority order. It sets only the fields it names, and `false` removes a field (`label = false`). `hide = "legend"` removes the main legend and keeps corners, glow, cooldowns and badges. Patches are how you change one thing without taking over the key.
- `badge`: a dot in a corner slot (`corner = "tl" | "tr" | "bl" | "br"`, default `tr`).
- `hide`: an empty key.

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

`bus` may also be a list (all must hold). The renderer subscribes to the topics conditions mention, so values from the computer arrive on the keyboard automatically.

## Signals: momentary looks

Some things are events, not states: a notification, a build failing, a timer ending. Anything can publish on `keys/signal` (not retained):

```json
{ "keys": ["KC_F5"], "set": { "glow": { "color": "#ff5c7c", "pulse": true } }, "duration_ms": 3000 }
```

The renderer applies `set` as a top-priority patch for `duration_ms`, then fades it out. `id` replaces an earlier signal with the same id, and `{ "id": ..., "clear": true }` ends it early. Selectors are the same as for rules (`key`, `keys`, `group`).

## Key Rules (the user's rules)

The `keys-custom` module ("Custom Keycap Visuals") becomes **Key Rules**. It is a list of rules in its config. Each row is the same rule shape, with keys, conditions and looks entered as friendly fields:

- Studio edits the list (pick keys on a keyboard map, conditions, look, live preview).
- `fluxctl modules config keys-custom --set rules='[...]'` edits the same data.

Existing per-key and per-action overrides keep working; they're expressed as patch rules.

## Performance

- Legends and corners are a cached 2D layer. The renderer redraws it only when providers, held keys, modifiers, layer, app, conditions or signals change. Glow, outline pulses and fades are GPU passes every frame and don't invalidate the cache.
- Conditions are re-evaluated on the events that can change them. Time and idle conditions are re-checked once a second.
