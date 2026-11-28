# Making Keyglass Modules

An Keyglass module is a plain folder on disk containing a manifest (`module.toml`), a Lua 5.4 script (`script.lua`), and optionally GLSL shaders (`shaders/*.glsl`) or a host companion script (`host/*.py`, `.js`, `.sh`, or any executable).

You do **not** need Rust, a compiler, or a checkout of the Keyglass source code to build, run, or distribute modules. Drop a folder into your local modules directory, edit files in your favorite editor, and Keyglass hot-reloads it live in Studio and syncs it straight to the keyboard.

---

## 1. Five-Minute Quickstart

### Where Local Modules Live

- **macOS / Linux:** `~/.local/share/keyglass/v2/modules/local/<your-module-id>/`
- **Tier priority:** `local` > `installed` > `bundled`. A local folder with the same ID as a bundled or installed module always wins, so upstream updates will never overwrite your edits.

You can create a folder by hand or scaffold one from Studio (**Modules → Create Local Module**) or the CLI:

```sh
fluxctl modules create my-key-flash --template effect --enable
```

### Minimal Example (`my-key-flash`)

Create two files inside `~/.local/share/keyglass/v2/modules/local/my-key-flash/`:

**`module.toml`**
```toml
id = "my-key-flash"
name = "My Key Flash"
version = "0.1.0"
description = "Soft glowing ring around every pressed key"
author = "Your Name"
category = "effect"
tags = ["key-press", "glow", "visual"]
stages = ["under_key"]
host_requirement = "none"

[entrypoints]
script = "script.lua"

[channels]
subscribes = ["input/keys/event"]
publishes = []

[[settings]]
key = "color"
label = "Ring Color"
kind = "color"
default = "#ff8844"

[[settings]]
key = "duration"
label = "Fade Time (s)"
kind = "number"
default = 0.6
min = 0.1
max = 2.0
```

**`script.lua`**
```lua
local flashes = {}

function on_event(topic, payload)
  if topic == "input/keys/event" and payload.kind == "press" then
    table.insert(flashes, {
      x = payload.x_px,
      y = payload.y_px,
      w = payload.w_px,
      h = payload.h_px,
      age = 0.0,
    })
  end
end

function on_update(dt)
  local dur = tonumber(module.config().duration) or 0.6
  local kept = {}
  for _, f in ipairs(flashes) do
    f.age = f.age + dt
    if f.age < dur then table.insert(kept, f) end
  end
  flashes = kept
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local dur = tonumber(module.config().duration) or 0.6
  local c = module.color("color", "#ff8844")
  for _, f in ipairs(flashes) do
    local t = f.age / dur
    local alpha = (1.0 - t) * 0.9
    ctx:key_glow("ring", f.x, f.y, f.w, f.h, 8.0 + t * 28.0, 3.5, alpha, { c[1], c[2], c[3], alpha })
  end
end
```

Save both files. Keyglass Studio detects the new folder within ~350 ms; enable it in Studio (or run `fluxctl modules enable my-key-flash && fluxctl keyboard sync`) and tap keys on your keyboard.

---

## 2. How the Runtime Works

Keyglass runs the **same** Lua 5.4 runtime and hierarchical pub/sub bus in two places:

1. **On the keyboard** (ARM64 RK3566 Linux): runs your `script.lua` and `.glsl` shaders directly at 60 FPS with zero PC needed (`host_requirement = "none"` or `"optional"`).
2. **On your computer** (macOS / Linux / Windows): runs Studio’s live preview plus any optional `entrypoints.host` companion script (Python, Node, shell, or native binary).

Both brokers mirror subscribed topics automatically over a full-duplex USB link (`OFV2`). When you save edits locally and sync, Keyglass uploads every file in your module folder (including subfolders like `lib/` and `shaders/`) directly to `/home/root/keyglass/v2/modules/<id>/` on the keyboard.

### Display Coordinates

- The Flux keyboard display is **1920 × 1080** pixels, origin `(0, 0)` at top-left.
- **Top bar:** roughly `y = 5..261` (split into 2, 3, or 4 slots by the active `top_bar_dock` module). Never hardcode slot pixels — call `module.slot_rect()`.
- **Key area:** rows 0–5 sit roughly from `y = 336` to `y = 1061`. Never hardcode key coordinates — read `keyboard.layout()`, `payload.x_px`, `payload.y_px`, or `key.visual_x` / `key.visual_y` (which include optical parallax offsets).

### Render Stages (Back to Front)

Declare every stage your `on_render(ctx, stage)` draws into in `module.toml`:

| Stage | Purpose | Typical `ctx` calls |
|---|---|---|
| `background` | Wallpaper, video, live stream, or procedural background shader | `ctx:background_media`, `ctx:stream_background`, `ctx:shader`, `ctx:background_dimming` |
| `distortion` | Post-processing / refraction over the live background & video | `ctx:shader` (reads `sample_scene(uv)`), `ctx:wave_params`, `ctx:wave_impulse` |
| `under_key` | Effects under keycaps, custom overlay shaders, keycap legends | `ctx:shader`, `ctx:key_glow`, `ctx:spawn_particles`, 2D draw calls, `ctx:keep_layer` |
| `tile` | Top-bar widgets clipped to `module.slot_rect()` | `ctx:clip`, `ctx:text`, `ctx:rect`, `ctx:round_rect`, `ctx:arc`, `ctx:circle`, `ctx:line`, `ctx:image` |
| `world_overlay` | Free-roaming companions or full-screen HUD overlays on top of everything | 2D draw calls (`ctx:round_rect`, `ctx:text`, `ctx:image`, etc.) |

---

## 3. `module.toml` Reference

```toml
id = "my-module"                   # lowercase alphanumeric + hyphens; matches folder name
name = "My Module"                 # display name in Studio
version = "1.0.0"                  # semver
description = "One-line summary"
author = "Your Name"
category = "effect"                # effect | widget | background | companion | host_service
tags = ["key-press", "visual"]     # primary tags: core, key-press, top-bar, background, keys, companion, shader
stages = ["under_key"]             # background | distortion | under_key | tile | world_overlay
host_requirement = "none"          # none (standalone) | optional (hybrid) | required (needs host process)
slot_capable = false               # true if widget can be placed in the Top Bar
stackable = true                   # false if mutually exclusive with same category

# Optional: claim a core capability role (lets forks/replacements cleanly replace built-ins)
provides = []                      # e.g. ["keys_renderer"] or ["top_bar_dock"]

# Optional: declare dependencies on module IDs or roles
depends = [
  # "some-module-id",
  # { role = "keys_renderer", min_version = "2.0.0", optional = false },
]

# Optional: for slot_capable = true widgets, declare which widths you actually lay out for
[top_bar]
supported_spans = [0.5, 1.0, 2.0]  # allowed values from: 0.5, 1.0, 2.0, 3.0, 4.0
default_span = 1.0
singleton = true                   # true = at most one instance on the Top Bar

# Optional: give this module its own top-level tab in Studio
[ui]
top_tab = false
top_tab_label = "My Tab"
top_tab_order = 50

[entrypoints]
script = "script.lua"              # optional Lua entrypoint
shader = "shaders/effect.glsl"     # optional primary shader (for disclosure)
host = "host/companion.py"         # optional host process (.py, .js, .sh, or native executable)

[channels]
subscribes = ["input/keys/event"]  # MQTT-style patterns (+ single level, # multi-level)
publishes = ["modules/my-module/status"]
schema_version = "1.0.0"           # optional semver for your published payloads

# Settings rendered automatically in Studio & configurable via `fluxctl modules config`
[[settings]]
key = "speed"
label = "Speed"
kind = "number"                    # number | boolean | string | color | select | file | key | list
default = 1.0
min = 0.1
max = 5.0
# options = ["mode_a", "mode_b"]   # for kind = "select"
# secret = true                    # stripped automatically when exporting shared presets!

# A list setting is a JSON array of rows; `fields` describes one row (scalar kinds only).
# Studio edits rows (add, remove, reorder); the CLI sets the whole array:
#   fluxctl modules config my-module --set buttons='[{"label":"Movie"}]'
[[settings]]
key = "buttons"
label = "Buttons"
kind = "list"
default = []
  [[settings.fields]]
  key = "label"
  label = "Label"
  kind = "string"
  default = ""

# Optional action buttons shown in Studio and triggerable via `fluxctl modules action`
[[actions]]
id = "pulse"
label = "Trigger Pulse"
topic = "modules/my-module/command"
payload = { action = "pulse" }
retain = false
```

### Key Actions Defined at Runtime (`dynamic_actions`)

`[[actions]]` are fixed in the manifest. A module whose actions come from its settings or the outside world (Stream Deck buttons, MQTT buttons) sets `dynamic_actions = true` and declares them while it runs, by publishing a retained message:

```json
// topic: system/actions/declared/<module-id>   (retain = true)
{ "actions": [ { "id": "movie", "label": "Movie", "topic": "modules/my-module/key",
                 "payload": { "button_id": "movie", "pressed": true },
                 "mode": "hold", "release_payload": { "button_id": "movie", "pressed": false } } ] }
```

Each action has the same fields as `[[actions]]`. Republish the whole list whenever it changes; a null payload withdraws it. Keyglass gives every action a sticky slot keyed by its id, so key bindings survive edits, restarts and reordering. Keep ids stable.

There are 472 module action slots, shared by every module. The first 56 work on any 2.x firmware; the rest need firmware 2.1.6 or newer (`requires_firmware` on the slot). Actions that don't get a slot are listed with a reason under `unassigned` on retained `system/actions`. `module_slots` shows `{ total, used }`.

> **Note on `kind = "file"` settings:** If a user picks a local video or image file on their computer for any `kind = "file"` setting, Keyglass automatically uploads it to `/home/root/keyglass/media/` when syncing to the keyboard and rewrites the keyboard’s copy to the remote filename.

---

## 4. Lua API Reference

### Lifecycle Callbacks (all optional)

```lua
function on_init() end                  -- called once on load / hot-reload
function on_event(topic, payload) end   -- called for every message matching channels.subscribes
function on_update(dt) end              -- called once per frame; dt is seconds (e.g. 0.016)
function on_render(ctx, stage) end      -- called once per declared stage in manifest.stages
```

### Multi-File Modules (`require`)

`package.path` automatically includes your module folder (`./?.lua`, `./?/init.lua`, `./lib/?.lua`) plus the shared library folder (`../_shared/?.lua`). You can split a module cleanly across files:

```lua
local mathx = require("lib.mathx")
```

### Global Helpers

- **`module.id()`** → module ID string.
- **`module.dir()`** → absolute path to this module’s folder on disk.
- **`module.config()`** → table of merged manifest defaults + user setting overrides.
- **`module.color(key, fallback_hex)`** → `{ r, g, b, a }` in `0.0..1.0` parsed from a `#rrggbb` setting.
- **`module.slot()` / `module.slot_pos()` / `module.slot_span()`** → assigned slot info (`nil` if unplaced).
- **`module.slot_rect()`** → `{ x, y, w, h }` in display pixels computed dynamically from the active top-bar dock (automatically reflects `0.5`, `1`, `2`, `3`, `4` slot spans).
- **`module.log(level, message)`** → writes to live Studio logs & `fluxctl modules logs <id>` (`"info"`, `"warn"`, `"error"`).
- **`bus.publish(topic, payload, retain)`** → publishes a Lua table/value onto the unified pub/sub bus (`retain` is boolean).
- **`bus.get(topic)`** → returns the current retained payload for an exact topic, or `nil`.
- **`bus.list(pattern)`** → returns a list of `{ topic = ..., payload = ... }` for all retained messages matching an MQTT wildcard pattern (e.g. `"keys/providers/#"`).
- **`keyboard.layout()`** → full physical geometry table (`width`, `height`, `header_rect`, `key_area_rect`, `slots`, `keys[]` where each key has `key_index`, `row`, `col`, `rect = {x,y,w,h}`, `center_x`, `center_y`, `visual_x`, `visual_y`, `parallax_dx`, `parallax_dy`).
- **`keyboard.nearest_key(x, y)`** → nearest key table for pixel `(x, y)`.
- **`keyboard.sample_sdf(x, y)`** → `{ dist, nx, ny, key_index }` signed distance field (`< 0` inside a keycap, `> 0` in the inter-key trench).
- **`json.decode(str)` / `json.encode(val)`** → fast JSON conversion.
- **`fs.list_dir(path)`** → sorted array of filenames in a directory.

### Render Context (`ctx`) Methods

All colors are `{ r, g, b, a }` with floats in `0.0..1.0`.

**2D Drawing (available in `under_key`, `tile`, `world_overlay`):**
- `ctx:clip(x, y, w, h)` / `ctx:clear_clip()`
- `ctx:rect(x, y, w, h, color)`
- `ctx:round_rect(x, y, w, h, radius, fill_color, stroke_width, stroke_color)`
- `ctx:circle(cx, cy, r, fill_color [, stroke_width, stroke_color])`
- `ctx:ellipse(cx, cy, rx, ry, color)`
- `ctx:line(x1, y1, x2, y2, width, color)`
- `ctx:arc(cx, cy, r, start_deg, end_deg, width, color)`
- `ctx:pie(cx, cy, r, start_deg, end_deg, color)` — filled sector (e.g. a clock wedge); `-90` is 12 o'clock, angles run clockwise.
- `ctx:text(x, y, str, size_px, color, align)` — `align` is `"left"`, `"center"`, or `"right"`.
- `ctx:image(x, y, w, h, path, tint_color)` — renders a PNG/JPEG file (relative paths resolve against `module.dir()`).
- `ctx:keep_layer()` — in `under_key`, tells the compositor nothing changed since last frame so it reuses the cached 2D overlay at zero CPU cost.

**GPU Effects & Media:**
- `ctx:background_media(path)` — plays a local video (`.mp4`/`.mkv`/`.mov` via RK3566 hardware decoder) or displays an image.
- `ctx:stream_background(stream_name)` — displays a live `CH_MEDIA` H.264/JPEG stream pushed to `POST /v2/streams/<stream_name>` full-screen as the background.
- `ctx:stream_rect(x, y, w, h, stream_name [, radius])` — renders a live `CH_MEDIA` hardware-decoded H.264 video stream inside a bounded popup or widget rectangle while keeping the normal background visible behind it.
- `ctx:background_dimming(frac)` — GPU-side background dimming (`0.0..0.9`, zero CPU cost).
- `ctx:top_fade(frac)` — GPU-side edge-to-edge top-bar tint dimming (`0.0..0.85`).
- `ctx:shader(rel_path, p0 [, p1, p2, p3])` — runs a custom `.glsl` shader in `background`, `distortion`, or `under_key` with up to four `vec4` uniforms (`p0..p3`, each `{x, y, z, w}`).
- `ctx:key_glow(shape, cx, cy, w, h, radius_px, thickness_px, intensity, color)` — GPU additive glow; `shape` is `"glow"`, `"ring"`, `"ripple"`, or `"solid"`.
- `ctx:wave_params(damping, speed, refraction, glint, tint)` / `ctx:wave_impulse(x, y, radius_px, force)` — drives the 60 Hz 2D water simulation.
- `ctx:spawn_particles(x, y, count, speed, spread_deg, radius_px, life_ms, color, bounce_keys, lens_refract)` — spawns SDF-aware bouncing & colliding particles.

---

## 5. Practical Recipes

### Recipe A — Responsive Top-Bar Widget (`0.5×`, `1×`, `2×`)

```lua
-- module.toml: slot_capable = true, stages = ["tile"], [top_bar] supported_spans = [0.5, 1.0, 2.0]
function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local r = module.slot_rect()
  if not r then return end

  ctx:clip(r.x, r.y, r.w, r.h)
  if r.w < 260 then
    -- Half-slot compact layout
    ctx:text(r.x + r.w * 0.5, r.y + 52, "OK", 38, { 0.4, 1.0, 0.7, 1.0 }, "center")
  elseif r.w > 580 then
    -- Widescreen 2-slot layout
    ctx:text(r.x + 20, r.y + 40, "BUILD STATUS: PASSING", 34, { 0.4, 1.0, 0.7, 1.0 }, "left")
  else
    -- Standard 1-slot layout
    ctx:text(r.x + r.w * 0.5, r.y + 56, "PASSING", 32, { 0.4, 1.0, 0.7, 1.0 }, "center")
  end
  ctx:clear_clip()
end
```

### Recipe B — Custom Keycap Faces / Macro Layer (Declarative Provider)

Instead of drawing directly over keys, publish rules to `keys/providers/<your-id>`. The active `keys_renderer` composites all providers atomically by priority with zero flicker and automatic parallax:

```lua
function on_init()
  bus.publish("keys/providers/my-macros", {
    priority = 45,       -- keymap = 10, shortcuts = 30, ability-deck = 50
    mode = "replace",    -- "replace" | "badge" | "hide"
    rules = {
      {
        key = "KC_F1",                       -- keycode name or physical index 0..84
        when = { app = "obs,zoom" },         -- optional app/profile/layer/fn predicate
        label = "Mute",
        icon = "mic",                        -- built-in vector icon, or `image = "icons/mute.png"`
        color = "#ff5c7c",
      },
      {
        key = "KC_C",
        mods = 8,                            -- bitmask: 1=Ctrl, 2=Shift, 4=Alt/Opt, 8=Cmd/Gui
        label = "Copy",
        icon = "copy",
        color = "#5ce1ff",
      },
    },
  }, true)
end
```

When your module is disabled or crashes, Keyglass’s Last-Will cleanup (`purge_origin`) removes `keys/providers/my-macros` automatically so keys revert cleanly.

Besides `label`, `icon`/`image` and `color`, a rule can carry extra detail. All of it is centered on the key or inset from its edge, because anything at a keycap's edge makes parallax and viewing-angle offsets obvious.

| Field | Effect |
|---|---|
| `glow = true` / `0.5` / `{ strength, color, pulse }` | Soft glow behind the icon (or legend) that fades in and out; `pulse` breathes |
| `corners = { tl, tr, bl, br }` | Inset corner slots, each text (superscript-style) or `{ text \| icon \| dot = true/false, color }` |
| `sub = "21°"` / `toggled = true/false` | Shorthands for top-right corner text / status dot |
| `cd`, `max_cd` | Cooldown as a clock sweep centered on the key; `cd_style = "curtain"` for the edge-to-edge curtain |
| `charges`, `max_charges` | Charge count in the top-right corner; recharge shows as a ring, or `progress_style = "edge"` for a bar along the top |
| `ult_pct` | Ring around the icon, percent in the top-right corner |
| `tile_bg = true` | Opt-in frame around the whole keycap |

Badge rules (`mode = "badge"`) draw a dot in the top-right slot, or `corner = "tl" | "bl" | "br"`.

### Recipe C — Custom Cross-Platform GLSL Shader (Works on Both Desktop & Keyboard!)

Write **one** `.glsl` file implementing `vec4 effect_main(vec2 uv)`. Keyglass wraps it automatically for both desktop `wgpu` (Metal/Vulkan/DX12) and the keyboard’s Mali-G52 GLES 2.0.

Built-in uniforms available automatically:
- `vec2 u_resolution` (`1920.0, 1080.0`)
- `float u_time` (seconds)
- `float u_dt` (frame delta seconds)
- `vec4 u_params[4]` (from `ctx:shader(path, p0, p1, p2, p3)`)
- `vec4 sample_scene(vec2 uv)` (in `distortion` and `under_key` stages, samples the live video/background rendered so far!)

**`shaders/heat_haze.glsl`** (stage = `"distortion"` — warps live video underneath!)
```glsl
vec4 effect_main(vec2 uv) {
  float strength = u_params[0].x; // e.g. 0.006
  float speed    = u_params[0].y; // e.g. 3.0
  vec2 offset = vec2(
    sin(uv.y * 28.0 + u_time * speed) * strength,
    cos(uv.x * 18.0 - u_time * speed * 0.7) * strength * 0.5
  );
  return sample_scene(uv + offset);
}
```

**`script.lua`**
```lua
function on_render(ctx, stage)
  if stage ~= "distortion" then return end
  local s = (tonumber(module.config().strength) or 6) * 0.001
  ctx:shader("shaders/heat_haze.glsl", { s, 3.0, 0.0, 0.0 })
end
```

### Recipe D — Host Companion Script & Live Video Streaming

Declare `host = "host/worker.py"` in `[entrypoints]`. Keyglass starts your process automatically when the module is enabled and restarts it if its settings change, injecting:

- `KEYGLASS_BUS_URL` — local daemon URL (e.g. `https://127.0.0.1:52323`)
- `KEYGLASS_CONFIG_JSON` — JSON object of the module’s current settings
- `KEYGLASS_MODULE_ID` / `KEYGLASS_MODULE_DIR`

Your script can publish to the bus (`POST /v2/bus/pub`), subscribe to keyboard events (`GET /v2/bus/events?pattern=input/keys/event` or `WS /v2/bus/ws`), or stream live hardware-decoded video straight to the keyboard via `POST /v2/streams/<name>` (`Content-Type: video/h264` chunked Annex-B, or `Content-Type: image/jpeg`). Anything printed to stdout/stderr appears in Studio’s live module log viewer.

---

## 6. Standard Bus Topics

| Topic | Retained | Payload Shape |
|---|---|---|
| `input/keys/event` | No | `{ key_index, kind: "press"\|"release", x_px, y_px, w_px, h_px, row, col, depth_um, velocity_um_s }`; `x_px, y_px` is where the key appears (parallax-corrected center) |
| `input/keys/analog` | No | `{ timestamp_ms, samples: [{ key_index, depth_um, velocity_um_s, x_px, y_px }] }` |
| `keyboard/state` | Yes | `{ caps_lock, num_lock, scroll_lock, active_layer_bitfield, idle_ms }` |
| `keyboard/keymap` | Yes | Active hardware keymap layers & `keyActions` |
| `host/app/active` | Yes | `{ app_id, process_name, window_title, profile_index, profile_id, profile_name }` |
| `host/system/metrics` | Yes | `{ cpu_percent, gpu_percent, memory_used_gb, memory_total_gb, vram_used_gb, net_up, net_down }` |
| `host/weather/current` | Yes | `{ city, temp, high, low, condition, is_day, aqi, sunrise, sunset, hourly: [...] }` |
| `host/media/now_playing` | Yes | `{ title, artist, album, playing, position_sec, length_sec }` |
| `fx/surface/impulse` | No | `{ x, y, radius, force, kind, source }` (shared by ripples, pets, timers) |
| `keys/providers/<id>` | Yes | `{ priority, mode, when?, rules: [...] }` |
| `modules/streams/<name>` | Yes | `{ active: true\|false, codec, name }` |
| `ui/toast` | No | `{ id, icon?, title, message?, level?, value_text?, color?, duration_ms? }` (renders via `ui-osd-toast`) |

---

## 7. Debugging, Testing & Sharing

- **Validate manifest & Lua syntax:**
  ```sh
  fluxctl modules validate ~/.local/share/keyglass/v2/modules/local/my-key-flash
  ```
- **Watch live logs & errors:**
  ```sh
  fluxctl modules logs my-key-flash
  ```
- **Simulate keypresses & capture a 1080p PNG frame without touching the keyboard:**
  ```sh
  fluxctl simulate key 49
  fluxctl preview capture --steps 10 --out /tmp/preview.png
  ```
- **Watch live bus traffic:**
  ```sh
  fluxctl bus sub "modules/my-key-flash/#"
  ```
- **Share with others:** Push your module folder to a Git repo (or zip/tarball it). Anyone can inspect and install it in Studio or via:
  ```sh
  fluxctl modules inspect github:yourname/my-key-flash
  fluxctl modules install github:yourname/my-key-flash --accept-disclosure
  ```
  Keyglass always shows a full disclosure report (files, shaders, host scripts, bus channels, and any update diffs) before installing or updating.

---

## Rich Metadata, Updates & Community Distribution

Include optional metadata fields at the top of `module.toml` so Keyglass Studio can display version history, support links, and update cards cleanly:

```toml
author = "Your Name"
license = "MIT OR Apache-2.0"
homepage = "https://github.com/you/my-keyglass-module"
repository = "https://github.com/you/my-keyglass-module"
support_url = "https://github.com/you/my-keyglass-module/issues"
discord_url = "https://discord.gg/your-invite"
changelog = "v1.1.0: Added custom accent color and smoother idle animation."
```

### Ways Users Can Install & Update Your Module

1. **Keyglass Official Registry (`keyglass-dev/keyglass-registry`):** Open a 1-line PR adding your repo to `sources.json`. GitHub Actions compiles a static `catalog.json` with SHA-256 verification.
2. **Custom Module Lists (“Taps”):** Publish your own static `catalog.json` (using `build-catalog.py` from `keyglass-registry`) on GitHub Pages; anyone can subscribe under **Manage Modules → Lists & Direct Install**.
3. **Direct Install (No List Needed):** Share `github:owner/repo` (or `github:owner/repo//subdir`). Keyglass remembers the origin for one-click updates and remembers it under **Previously Removed Modules** if uninstalled.
4. **Deprecating, Renaming, or Moving Later:** Leave `deprecated = true`, `deprecation_message = "..."`, `moved_to = "..."`, and optionally `replaced_by = "..."` in your old `module.toml` so existing users get a clean 1-click **Migrate** prompt that preserves their settings.

For the full submission checklist, custom catalog setup, and requesting a catalog be added by default, see [docs/submitting-modules-and-catalogs.md](submitting-modules-and-catalogs.md).

### Forking Any Module to Local

Want to tweak a built-in or installed module?
- Click **Fork to Local** inside the module’s detail view (or run `fluxctl modules fork <id> <new-id>`).
- Keyglass copies the module into `~/.local/share/keyglass/v2/modules/local/<new-id>/`.
- Edit `script.lua` or `module.toml` with any editor; Keyglass hot-reloads on save, and `fluxctl keyboard sync` pushes your changes straight to the keyboard.

### Profiling Latency & Using the Built-In MCP Server

- **Terminal Profiler:** Run `fluxctl perf --watch` to inspect real-time FPS, frame interval `p50/p95/p99/max`, >22 ms hitch causes, and per-module Lua execution timings (`avg_us` / `max_us`).
- **AI / IDE MCP Server:** Add `fluxctl mcp` as a stdio MCP server in Pi, Claude Code, or Cursor to expose `keyglass_status`, `keyglass_perf`, `keyglass_modules_list`, `keyglass_module_logs`, `keyglass_bus_retained`, `keyglass_bus_publish`, and `keyglass_keyboard_sync`.
