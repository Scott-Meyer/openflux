# Profiles

`v2.0.0` · **Core Built-In** · **Beta** · Sandboxed Lua (runs on keyboard)

First-class context & profile engine with inheritance fall-through, per-profile module overrides, multi-computer host awareness, Top Bar Touch Bar layouts, and automatic QMK keybinding layer switching

## Bindable Key Actions

Bind any of these actions to a physical key in **Keyglass Studio → QMK Keymap → Module Actions**:

- **Next Profile** (`next`, tap)
- **Previous Profile** (`prev`, tap)
- **Auto Profile Mode** (`auto`, tap)

## Runtime & Permissions

- **Sandboxed Lua 5.4 only** — runs directly on the keyboard with a 150 ms execution watchdog and no shell or filesystem write access.

## Installing & Customizing

- Ships built into **Keyglass Studio**. Toggle it under **Manage Modules**, or click **Fork to Local** to customize its code under `~/.local/share/keyglass/v2/modules/local/`.
