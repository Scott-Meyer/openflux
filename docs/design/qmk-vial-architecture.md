# QMK & Vial Integration Plan

Move the keyboard's keymap and typing behavior onto **real upstream `vial-qmk`** and an **embedded Vial web UI**, instead of maintaining or reimplementing a custom keymap engine.

Typing stays 100% on the RP2040 microcontroller—independent of the Linux display computer—while unlocking the full QMK feature set (home-row mods with proper buffering, layers, one-shot mods, combos, tap dance, key overrides, Caps Word, macros, `.vil` sharing).

```text
  Hall-effect scanner (existing C)
         │  key_pressed[]
         ▼
  Real vial-qmk (quantum/ + vial.c) inside Pico SDK shell
    ├── USB IF 0 (HID) ──────────────► OS typing (unchanged)
    ├── USB IF 5 (Vial Raw HID) ◄────► vial.rocks / vial-gui / openfluxd ◄──► Studio tab (vialite)
    ├── USB IF 4 (Vendor HID) ───────► openfluxd (OS-side module actions)
    └── UART1 HDLC ──────────────────► Linux display (real mods/layers, key faces, keyboard module actions)
```

---

## 1. Firmware: Real `vial-qmk` Inside the Existing Pico SDK Shell

Keep the working RP2040 hardware shell untouched:
- Hall-effect sensing (`fw_depth.c`, `fw_refine.c`, `fw_rest.c`, `fw_thermal.c`, `fw_rapid.c`, `fw_keyframe.c`)
- Factory LittleFS calibration partition at `0x00F00000`
- TinyUSB interfaces `IF 0` (NKRO/Boot/Consumer), `IF 1` (WinUSB bulk commands for `openfluxd`), `IF 2/3` (CDC logs), `IF 4` (Vendor HID)
- UART1 HDLC link and Linux SoC power/reset GPIOs

Replace only the ~200-line post-scan keycode resolver in `fw_scanner.c` by compiling vendored `vial-qmk` (`quantum/` + `vial.c`) behind QMK's 5 platform hooks:

| QMK Hook | Wired To |
|---|---|
| `timer_read` / `timer_read32` | Pico SDK millisecond clock |
| `matrix_scan` | `rt->key_pressed[]` from the Hall-effect scanner |
| `host_driver_t` (HID reports) | Existing TinyUSB `IF 0` keyboard/consumer endpoints |
| `raw_hid_send` / `raw_hid_receive` | New TinyUSB `IF 5` Vial Raw HID (`0xFF60:0x61`, 32-byte IN/OUT) |
| `eeprom_read_block` / `eeprom_write_block` | RAM buffer flushed (debounced) to a per-layout LittleFS file (ANSI/ISO/JIS), separate from calibration |

**Hall-effect guard:** Rapid trigger micro-bounces fight tap-hold timers. Whenever QMK says the active binding on key `k` is a tap-hold or tap-dance key, disable rapid trigger on that key.

**Clean split of ownership:**
- **QMK/Vial owns keymaps** (standard 16-bit QMK/Vial format in LittleFS).
- **OpenFlux owns actuation, profiles, and display.** No custom 32-bit keymap format or two-way keymap sync between Studio and Vial.

*Why not a full ChibiOS `vial-qmk` port?* Porting TinyUSB's WinUSB bulk interface (`IF 1`), MS OS 2.0 descriptors, LittleFS calibration protection, UART1 HDLC, and RAM-resident ADC muxing onto ChibiOS adds heavy low-level churn for no benefit over keeping Pico SDK and linking `vial-qmk`'s C core directly.

---

## 2. Module Actions (OS-Ignored Keys) via Dynamic `vial.json`

Keyboard-side Lua modules (like `widget-pomodoro`) and OS-side modules both need bindable actions that show up in the keymap UI and never emit OS keystrokes.

Vial already supports this natively through `customKeycodes` in `vial.json`:

1. **Modules declare actions in `module.toml`:**
   ```toml
   [[actions]]
   id = "widget-pomodoro.toggle"
   name = "Pomo\nToggle"
   title = "Start or pause Pomodoro timer"
   ```
2. **`openfluxd` maintains a stable slot map (`slot -> action_id`):**
   - Built-in Flux functions (brightness, profile/theme/actuation cycling, etc.) occupy the first slots; module actions get persistent slots after them so enabling/removing modules never shifts existing bindings.
   - Whenever modules change, `openfluxd` generates the tiny XZ-compressed `vial.json` (~1–2 KB) with `"customKeycodes"` populated from those names/tooltips and writes it to LittleFS on the RP2040 (firmware falls back to a compiled-in default if none exists yet).
3. **Every Vial UI shows them automatically:**
   - When `vialite`, `vial.rocks`, or `vial-gui` connects, it reads `vial.json` from the keyboard and lists all declared actions under the **User** tab—ready to place on any key, layer, combo, or tap dance with zero custom UI code.
4. **Firmware stays dumb about modules:**
   - In `process_record_user()`, any `QK_KB_*` custom keycode returns `false` (no OS keystroke) and emits `{ slot, pressed }` on both:
     - **UART1 → `openflux-display`:** resolves `slot -> action_id` from the keyboard's local slot map and publishes on the keyboard bus (works even with no PC daemon running).
     - **USB Vendor HID `IF 4` → `openfluxd`:** resolves `slot -> action_id` and publishes on the host bus (no macOS Input Monitoring permissions needed).

*(Verify exact `QK_KB` / `QK_USER` slot range against the pinned `vial-qmk` commit and `vialite` during bring-up; modern QMK uses 32 `QK_KB` + 32 `QK_USER` slots.)*

---

## 3. Live QMK State for On-Keyboard Legends

Today `modules/keys-renderer/script.lua` guesses Shift/Ctrl from physical key positions on Layer 0, which breaks with home-row mods and one-shot modifiers.

- **Active state in `MSG_UPDATE`:** Pack QMK's actual `get_mods()`, `get_oneshot_mods()`, `get_oneshot_layer()`, `is_caps_word_on()`, `layer_state`, and `default_layer_state` into the 34 unused zero bytes of the existing 139-byte UART `MSG_UPDATE` frame.
- **Resolved key faces on change:** When the active layer or keymap changes, send the current layer's resolved QMK keycodes over UART so keycap legends always match the keyboard—even after editing in external `vial.rocks` without `openfluxd`.

---

## 4. Visual UI: Embedded `vialite` Tab in Studio

Embed **`tcyeee/vialite`** (React + TypeScript + Vite—the same stack as `host/studio`) as a top-level Studio tab.

- **Tauri / macOS WebView transport:** Tauri on macOS uses WKWebView, which lacks WebHID (`navigator.hid`). Since `openfluxd` already owns USB/HID natively in Rust, replace `vialite`'s thin WebHID layer with a 32-byte Vial packet relay through `openfluxd`.
- **Result:** Works identically on macOS, Windows, and Linux with no HID contention, gives full UI support for layers, mod-tap, combos, tap dance, key overrides, macros, QMK settings, and `.vil` import/export—and external `vial.rocks` (Chrome) or `vial-gui` also work directly via USB `IF 5`.
- When modules change in Studio, tell the Vial tab to reload `vial.json` so newly declared actions appear immediately.

---

## 5. Build Order

1. **Firmware QMK + Vial bring-up:** Vendor `vial-qmk`, wire the 5 Pico SDK shims + `IF 5` Vial Raw HID + debounced LittleFS EEPROM + rapid-trigger guard on dual-role keys. Verify typing, home-row mods, and live editing from `vial.rocks` / `vial-gui`.
2. **Display state over UART:** Populate `MSG_UPDATE` with QMK's real mod/layer state, send resolved keycodes on layer/keymap change, and update `keys-renderer/script.lua` to stop guessing modifiers from Layer 0.
3. **Dynamic module actions:** Add `[[actions]]` to `module.toml`, stable slot mapping + XZ `vial.json` write to RP2040 LittleFS, and `process_record_user()` forwarding over UART + `IF 4` Vendor HID.
4. **Embedded Studio UI:** Add `openfluxd` 32-byte Vial HID relay and mount `tcyeee/vialite` in a Studio top tab, retiring the old custom keymap editor.

---

## Intentionally Skipped

- No clean-room QMK reimplementation or host C++ QMK "test oracle" (we run real `vial-qmk`).
- No custom 32-bit `keymap 1.1` format or Vial-to-native translation/sentinel layer (standard QMK/Vial owns keymaps).
- No custom QMK editors (Tap Dance / Combos / Key Overrides / QMK Settings) built from scratch in Studio (`vialite` provides them).
- No `tap_hold_pending_bitmap` animation over UART or speculative depth-aware tap-hold plumbing.
