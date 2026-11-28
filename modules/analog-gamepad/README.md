# Analog Gamepad

`v2.0.0` · **Core Built-In** · **Beta** · Sandboxed Lua (runs on keyboard)

Turns Hall-effect keys bound to GP_* keycodes in QMK Keymap into 1000 Hz analog controller sticks, triggers, and buttons (Off / Classic HID for macOS, Linux & Steam / XInput) with deadzone, response curves, and automatic per-game App Linking

## Settings

- **USB Gamepad Mode (Off = No Phantom Controller)** (`mode`, select)
- **Deadzone (mm)** (`deadzone_mm`, number)
- **Full Press / Saturation (mm)** (`saturation_mm`, number)
- **Analog Response Curve** (`curve`, select)
- **Circular Radial Stick Clamp (Normalize Diagonals)** (`radial_clamp`, boolean)
- **App Linking — Auto-Switch Layer for Games (comma-separated)** (`linked_apps`, string)
- **App Linking — Target QMK Layer (0..7)** (`linked_layer`, number)

## Bindable Key Actions

Bind any of these actions to a physical key in **Keyglass Studio → QMK Keymap → Module Actions**:

- **Cycle Gamepad Mode (Off / HID / XInput)** (`cycle_mode`, tap)

## Runtime & Permissions

- **Sandboxed Lua 5.4 only** — runs directly on the keyboard with a 150 ms execution watchdog and no shell or filesystem write access.

## Installing & Customizing

- Ships built into **Keyglass Studio**. Toggle it under **Manage Modules**, or click **Fork to Local** to customize its code under `~/.local/share/keyglass/v2/modules/local/`.
