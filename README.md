# Keyglass

> [!IMPORTANT]
> ## Keyglass is an independent project, not affiliated with or endorsed by Flux.

Keyglass is open-source, modular software for the [Flux keyboard](https://fluxkeyboard.com) — the Hall-effect keyboard with a 1920×1080 display beneath clear keycaps and an onboard Linux computer.

## Why Keyglass Is Different

Stock Polymath software ships as a single closed application. **Keyglass is built as a modular keyboard OS.**

Every background, keypress animation, keycap legend layer, top-bar widget, and computer integration is an independent module. Core essentials ship built in; everything else installs, updates, or forks on its own from community catalogs or direct GitHub repos without waiting on an app release. You only run the pieces you actually want, and you can inspect or edit any module yourself.

## What Keyglass Gives You (Including Features Beyond Polymath)

- **Native QMK / Vial Keybindings & Layers:** Full 8-layer QMK/Vial keymaps edited live right inside Keyglass Studio — drag-and-drop bindings, Tap/Hold (`MT`/`LT`), Tap Dance, modifier combos, and 64 custom module action slots. Converts cleanly from stock keyboards (preserving your existing Polymath keybindings, calibration, and optional 1.5 press-refinement typing weights) while keeping optical-parallax-aligned legends on every key.
- **Analog Gamepad Input *(Beta)*:** Turn any Hall-effect keys into 1000 Hz analog controller sticks, triggers, and buttons over Classic USB HID, with deadzone, saturation, response curves, and per-game layer switching.
- **Reactive Key Animations & Hardware Video Backgrounds:** Key-bouncing sparks that collide with your parallax-aligned keycap borders, 3D viscosity water ripples, classic rings and glows, custom GLSL shaders, and smooth hardware-decoded 1080p looping video backgrounds.
- **Modular Information Area & Physical Top Attachments:** Mix and match half-width or multi-slot top-bar widgets — Clock, Weather, System Load, Now Playing, Pomodoro, 3D printer status with live RTSP camera glance, and volume/notification HUD toasts — while automatically dodging physical snap-on Top Attachments (Rotary Dials, Sliders, Macro Bars, and Opaque Covers).
- **Profiles, Multi-Computer Contexts & Stream Deck Plugins *(Beta)*:** Inheritable profile chains, automatic per-app switching with conflict diagnostics, Touch Bar top-bar swaps, and support for running real Elgato `.streamDeckPlugin` / `.sdPlugin` actions directly on your keycaps.
- **Safe, Transparent & Reversible:** Sandboxed Lua 5.4 runtime with a 150 ms watchdog, mandatory pre-install permission disclosure (`Sandboxed` vs. `Host Helper`), honest `Alpha` / `Beta` / `Deprecated` / `Abandoned` maturity badges, full CLI/API/MCP parity (`fluxctl`, `fluxctl perf`, `fluxctl mcp`), and **1-click Return to Stock** that keeps factory calibration untouched.

## Getting Started

1. **Install Keyglass Studio:** [docs/installing.md](docs/installing.md)
2. **Convert your Flux keyboard (or return to stock anytime):** [docs/converting-a-keyboard.md](docs/converting-a-keyboard.md)
3. **Add or build modules:**
   - Browse first-party optional modules in [`keyglass-dev/keyglass-modules`](https://github.com/keyglass-dev/keyglass-modules) and the default catalog in [`keyglass-dev/keyglass-registry`](https://github.com/keyglass-dev/keyglass-registry).
   - Write or fork your own module: [docs/making-modules.md](docs/making-modules.md)
   - Publish a module, host your own catalog (“Tap”), or deprecate/rename a module: [docs/submitting-modules-and-catalogs.md](docs/submitting-modules-and-catalogs.md)

## More Docs

- [Architecture Overview](docs/architecture.md)
- [Component Compatibility](docs/compatibility.md)
- [Releasing](docs/releasing.md)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. See [NOTICE](NOTICE) for third-party licenses including QMK/Vial (GPL-2.0-or-later), FFmpeg, libusb, Lua 5.4, and Montserrat (SIL OFL).
