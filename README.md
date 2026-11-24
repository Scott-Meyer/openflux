# Keyglass

> [!IMPORTANT]
> ## Keyglass is an independent project, not affiliated with or endorsed by Flux.

Keyglass is open-source, modular keyboard software for the [Flux keyboard](https://fluxkeyboard.com) — a Hall-effect keyboard with a 1920×1080 display beneath clear keycaps and an onboard Linux SoC. It covers the full stack: RP2040 QMK/Vial + Analog Gamepad firmware, the RK3566 Wayland/EGL modular compositor, the `OFV2` priority-multiplexed USB bridge, and **Keyglass Studio** on your desktop.

## What Keyglass Does

- **Modular Keyboard OS:** Every visual effect, top-bar widget, keycap legend provider, background, and integration is a hot-reloadable module running on a hierarchical pub/sub bus (`openflux-bus`). Core essentials ship built in; optional and community modules install and update independently from [keyglass-modules](https://github.com/keyglass-dev/keyglass-modules), the [keyglass-registry](https://github.com/keyglass-dev/keyglass-registry), third-party module lists (“Taps”), or direct GitHub repos.
- **Native QMK/Vial Keymap & Hall-Effect Analog Gamepad:** Edit all layers, Tap/Hold (`MT`/`LT`), Tap Dance, 64 unified module action slots (`0x7E00..0x7E3F`), and 1000 Hz Hall-effect Analog Gamepad bindings directly in Keyglass Studio — with live optical-parallax-aligned keycap legends on the keyboard.
- **Profiles & Multi-Computer Contexts:** Inheritable profiles (fall-through chains), per-app QMK layer switching, dynamic Touch Bar top-bar overrides, and per-host keyboard backups.
- **Hardware-Accelerated Display & Top Attachments:** RKMPP hardware video decoding, custom GLSL shaders, 2D wave refraction & Key-SDF particles, optical parallax calibration (shared across legends and physics), automatic computer display sleep matching, and physical Top Attachment awareness (Clear Window, Rotary Dial, Slider, Macro, Opaque Cover).
- **Safe, Reversible & Transparent:** Sandboxed Lua 5.4 execution with a 150 ms watchdog, mandatory pre-install permission disclosure (`Sandboxed Lua` vs. `Host Helper`), bearer-authenticated local API, and 1-click **Return to Stock** whenever you want factory software back.

## Developer & Module Authoring Tools

- **Author & Fork Modules:** See [docs/making-modules.md](docs/making-modules.md) for creating local modules, forking any installed module to a local copy, and publishing to a Registry or custom list.
- **Live Latency Profiler:** Run `fluxctl perf` (or `fluxctl perf --watch`) for live FPS, p50/p95/p99 frame intervals, hitch breakdown, RKMPP queue telemetry, and per-module Lua execution timings (`µs`).
- **Built-In MCP Server:** Point Pi, Claude Code, Cursor, or any MCP client at `fluxctl mcp` for live status, performance profiling, module logs, bus inspection, and keyboard sync.

## Documentation

- [Installing Keyglass Studio](docs/installing.md)
- [Converting a Flux Keyboard & Returning to Stock](docs/converting-a-keyboard.md)
- [Making & Publishing Modules](docs/making-modules.md)
- [Architecture Overview](docs/architecture.md)
- [Component Compatibility](docs/compatibility.md)
- [Releasing](docs/releasing.md)

## License

Dual-licensed under the [MIT License](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option. See [NOTICE](NOTICE) for third-party licenses including QMK/Vial (GPL-2.0-or-later), FFmpeg, libusb, Lua 5.4, and Montserrat (SIL OFL).
