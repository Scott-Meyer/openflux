# QMK-Style Keymaps, Flux Module Events, Legend State & Visual UI Architecture

This document captures the architectural plan for adding QMK-style keymap behavior (layers, mod layers, mod-tap, layer-tap, one-shot modifiers/layers, combos, tap dance, key overrides) to the OpenFlux keyboard while preserving Hall-effect sensing, routing Flux-only actions to PC and keyboard modules, exposing QMK's internal state to on-keyboard legends, and using an existing visual UI for configuring/saving key setups.

---

## 1. Goals and Verified Codebase Baseline

### What we actually want
1. **Hall-effect key sensing stays intact** — magnetic calibration, thermal/rest tracking, fixed actuation, and rapid trigger keep working.
2. **QMK-style keymap features** — layers, mod layers, mod-tap, layer-tap, one-shot modifiers/layers, combos, tap dance, key overrides, macros, Caps Word.
3. **Flux-only key events** — keys (including tap/hold/combo outcomes) that do **not** emit OS keystrokes, routed on the internal channel to keyboard-side modules and to PC-side software, with a clear way to know and assign them.
4. **Live internal state on keyboard legends** — state that QMK does not send to the host computer (resolved mod-tap holds, armed one-shot modifiers, active/one-shot layers, Caps Word, pending tap-hold) is visible to on-keyboard modules so keycap legends can show live indicators.
5. **Existing visual UI** — configure and save new key setups without inventing a complex QMK visual programmer from scratch.

### Current codebase baseline (observed facts)
- **Firmware (`keyboard/firmware/src/`):** RP2040, C, Pico SDK + TinyUSB (~147 KB binary, 16 MiB flash with LittleFS calibration at `0x00F00000`).
- **Sensor-to-action seam (`fw_scanner.c`):** `fw_runtime_process_scan()` runs the Hall-effect stack first (`scan_classic` / `scan_refined`) to produce `rt->key_pressed[k]` and `rt->flux[k]`, then resolves keycodes in lines ~770–950 (`rt->latched_action[k]`). Unknown `kind` nibbles are silently ignored as no-ops.
- **Dynamic keyframe layout (`fw_keyframe.c`):** Magnetically detects attached ANSI (85 keys), ISO (87 keys), or JIS (90 keys) keyframe and switch feel.
- **USB (`usb_descriptors.c`):**
  - IF 0 (`EP 0x83`): NKRO Keyboard + Boot fallback + Consumer Control.
  - IF 1 (`EP 0x01`/`0x84`): OpenFlux vendor bulk commands (`SEND_KEYMAP`, `FILE_FROM_HOST`, `ENTER_BOOTLOADER`, `ENTER_OS_BOOTLOADER` SoC reset, `PUT_PRESS_REFINEMENT`). MS OS 2.0 WinUSB descriptors bind specifically to `ITF_NUM_VENDOR` (interface 1).
  - IF 2/3: CDC ACM logs.
  - IF 4 (`EP 0x85`): Vendor HID (`0xFF00:0x01`, `wMaxPacketSize = 8`, no Report ID), sending a raw 4-byte mask (`rp2040_main.c`). `host/daemon/src/usb/function_keys.rs` reads it without macOS Input Monitoring permissions and matches `[a, b, c, d]` or `[3, a, b, c, d, ..]` (`REPORT_ID = 3`).
- **UART1 (`fw_protocol.h`, `fw_hdlc.h`, `keyboard/bridge/src/uart_bridge.rs`):** 921,600-baud HDLC (`FW_HDLC_MAX_LEN = 512`) to `/dev/ttyS3`. `MSG_UPDATE` is 139 bytes with unused zero bytes at `0x52..0x5B` (10 bytes) and `0x6C..0x83` (24 bytes). Old bridges drop unknown UART message types without replying; `rp2040_main.c` refreshes bridge connectivity whenever any valid response arrives.
- **Display & Studio (`keyboard/display/src/v2_runtime.rs`, `modules/keys-renderer/script.lua`, `host/studio/`):**
  - `keys-renderer/script.lua` currently guesses modifiers from physical `key_index` via a static Layer 0 `MOD_KEYS` table and collapses `active_layer_bitfield` to one integer.
  - `host/studio` is React 18 + TypeScript + Vite + Tauri (`MIT OR Apache-2.0`, enforced by `deny.toml` on Rust crates), and already has `KeymapStylemapEditor.tsx`, `KeycodePickerModal.tsx`, `MacroEditorPanel.tsx`, and `ActuationPanel.tsx`.

---

## 2. Recommended Architecture & Decisions

### Decision 1: Keep Pico SDK + TinyUSB + Hall-Effect Engine; Replace Only the Post-Scan Action Layer

Keep `rp2040_main.c`, `fw_keyframe.c`, `fw_classic_*`, `fw_depth.c`/`fw_refine.c`, `fw_rest.c`/`fw_thermal.c`, `lfs_store.c`, `fw_usb_cmd.c`, and `fw_hdlc.c`. Replace only the keymap/action resolver in `fw_scanner.c` with a QMK-style action engine (`fw_qmk_engine.c`) and a Vial protocol handler (`fw_vial.c`).

**Why:**
- Preserves factory LittleFS calibration layout (`0x00F00000`, copy-on-write/power-loss safe—low risk when storing keymap data in separate LittleFS files), magnetic keyframe hot-swap, thermal/rest recovery, UART1 telemetry, and SoC power/reset GPIOs.
- Keeps existing USB interfaces (`IF 0..4`) and `compat.toml` compatibility intact.
- Can be built and tested on the host with ASan/UBSan (`make -C keyboard/firmware test`).

**Correctness mitigation (Test-only QMK oracle):**
Tap-hold timing interactions (`MT`, `LT`, `PERMISSIVE_HOLD`, `HOLD_ON_OTHER_KEY_PRESS`, `CHORDAL_HOLD`, `FLOW_TAP_TERM`, combos, one-shot mods) are the riskiest part to hand-write. Build a **test-only host oracle** by fetching upstream QMK at a pinned commit in test tooling (never vendored into `keyboard/firmware/src/` or shipped in binaries) and building QMK's C++/gtest action layer (`tests/`) alongside `fw_qmk_engine.c` to replay identical timed boolean event scripts through both engines.
- *Scope note:* The oracle validates agreement on scripted boolean-press scenarios; analog depth input and the rapid-trigger dual-role guard are tested separately by our own firmware tests.

### Decision 2: Native 32-Bit Keymap & Dual-Role Table (`keymap 1.1`); Vial 16-Bit Only at `fw_vial.c`

Do **not** adopt QMK's 16-bit keycodes as the firmware's native format, and do **not** put QMK 16-bit `MT`/`LT` words inside native storage:
- In QMK's 16-bit encoding, `MT(mod, kc)` and `LT(layer, kc)` only allow a basic 8-bit keycode (`0x00..0xFF`) in the tap slot—they **cannot** express "tap for a Flux module action, hold for Shift" or "tap for a consumer control key".
- OpenFlux's 32-bit format (`docs/protocol/keymap-format.md`) already encodes arbitrary 16-bit Consumer Page `0x0C` usages (`0x4`), an 8-bit modifier mask on any key (`0x0`), `KC_TRANSPARENT = 0x50000000`, and `KC_NO = 0xF0000000`.
- **Native extension (`keymap 1.1`):**
  - Keep existing `0x0..0x5` and `0xF` word kinds unchanged.
  - Add kind **`0x6`** for **native dual-role entries**: indexes a per-section dual-role table where each entry stores full 32-bit `tap_word` and `hold_word` actions (so any key, consumer usage, macro, layer action, or Flux module action can sit in either tap or hold), plus per-entry options/tapping-term override.
  - Add kind **`0x7`** for inline QMK-style single-word actions (`OSM`, `OSL`, `TT`, `TD(idx)` into a tap-dance table, `Caps Word`), plus section tables for combos, key overrides, and QMK timing settings.
- **Vial boundary (`fw_vial.c`):**
  - Vial's 16-bit keycode space (`QK_KB` `0x7E00..0x7E3F` = 64 codes, `QK_USER` `0x7E40+`, `QK_MACRO` `0x7700..0x777F`) exists **only** inside `fw_vial.c` as a translated view onto the single on-device store.
  - **Vial preservation rule:** Any native 32-bit word (or dual-role entry) that Vial's 16-bit space cannot represent (e.g., `MT(LSft, FLUX_MOD_1)` or an unmapped `0x4` consumer usage) is exposed to Vial as a dedicated read-only placeholder `QK_KB` sentinel (`OF_NATIVE`). Writing that placeholder back from Vial **leaves the original 32-bit word untouched** so Vial never silently destroys native bindings it cannot display.
  - **Keymap ownership:** Studio writes native 32-bit keymap blobs over IF 1 (as today); Vial is a translated read/write view onto that same on-device store; the firmware rewrites the blob/store itself so keycodes and actuation tables stay together without two writers.
- **Flash write safety (`lfs_store.c`):**
  `rp2040_lfs_erase()` and `rp2040_lfs_prog()` disable interrupts during writes. Vial's one-keycode-per-packet writes must mutate **RAM immediately** and debounce-flush to LittleFS after a write-quiet period (or explicit commit) so ADC scanning and USB never stutter.

### Decision 3: Pass `(pressed, depth_um)` Into the Action Engine & Guard Dual-Role Keys

`fw_rapid.c` and `fw_classic_keys.c` generate release/re-press edges from small partial travel reversals. On a mod-tap or layer-tap key, that triggers `QUICK_TAP_TERM` / tap-repeat behavior in ways mechanical switches never experience.

- **Default rule:** Disable rapid trigger (or require full hysteresis) on any key whose active-layer binding is dual-role (`0x6` / Tap Dance), unless explicitly overridden.
- **Event interface:** Pass `(key_idx, pressed, depth_um, now_ms)` into `fw_qmk_engine.c` rather than booleans alone, leaving room for depth-aware tap-vs-hold rules later.

### Decision 4: Flux-Only Module Events (OS-Ignored) Across USB IF 4 and UART

Flux actions (`0x5000_xxxx` natively, mapped to `QK_KB_0..63` in Vial) work inside dual-role keys, combos, and tap dance and **never** emit to IF 0 keyboard/consumer HID.

- **QK_KB slot assignment (64 slots in Vial):**
  - `QK_KB_0` (`0x7E00`): `OF_NATIVE` (read-only placeholder for non-Vial-representable native words)
  - `QK_KB_1` (`0x7E01`): `Caps Word` (or reserved; `KC_OF_*` codes `2..15` sit at `QK_KB_2..15` (`0x7E02..0x7E0F`) matching `fw_protocol.h`!)
  - `QK_KB_16..63` (`0x7E10..0x7E3F`): 48 Vial-visible generic Flux module slots (`FLUX_MOD_0..47`); native `0x5000_xxxx` words allow additional slots in Studio.
- **PC side (`IF 4`, `0xFF00:0x01`, `EP 0x85`):**
  - Keep `ITF_NUM_VENDOR = 1` unchanged for WinUSB; append Vial Raw HID at interface 5 (`ITF_NUM_HID_VIAL`).
  - Use **Report ID 3** for the legacy 4-byte mask (`function_keys.rs` already matches `[3, a, b, c, d, ..]`) and **Report ID 4** for the generic action report (`{ report_id=4, action_id: u16, kind: press|release, mods: u8, layer: u8 }`—`press`/`release` is sufficient since QMK already resolves tap vs. hold into distinct target keycodes), and raise `EPNUM_HID_VENDOR` `wMaxPacketSize` from `8` to `16` (or `64`).
  - `openfluxd` publishes decoded actions to host `openflux-bus` (`input/flux/action`) with zero OS Input Monitoring permissions.
- **Keyboard display side (`UART1`):**
  - Add a dedicated **`FW_UART_MSG_FLUX_ACTION = 5`** frame (mirroring `FW_UART_MSG_ANALOG = 4`), without expecting a bridge ACK so old bridges drop it safely.
  - `openflux-bridge` forwards it to `openflux-display`, which publishes `input/flux/action` on the keyboard's local `openflux-bus`.
- **Slot ownership & discovery:**
  - Modules declare named actions in `module.toml` (`[[actions]]` with `id`, `label`, `icon`, `target`).
  - Sticky/tombstoned slot assignments (`slot_id -> action_id`) live on the keyboard alongside the keymap (synced by `openfluxd` with keymap saves) so toggling modules never shifts existing bindings and keyboard-side modules work even without a host daemon.

### Decision 5: Stream QMK Internal State & Resolved Key Faces to the Display

Stop inferring modifiers and layers in `modules/keys-renderer/script.lua` from `key_bitmap` and Layer 0 `MOD_KEYS`:

1. **Live QMK state in `MSG_UPDATE` (`uart 1.1`):**
   Use the 34 unused zero bytes (`0x52..0x5B` and `0x6C..0x83`) in the existing 139-byte `MSG_UPDATE` frame:
   - `0x52` (`u8`): `qmk_state_ver = 1` (capability marker; `0` means fields below are not populated)
   - `0x53` (`u8`): `effective_mods` — **host-visible + held modifiers** (`get_mods() | embedded_0x0_key_mods`), so legends reflect both active mod-tap holds and modifier-wrapped keys
   - `0x54` (`u8`): `oneshot_mods` (`get_oneshot_mods()`, stays set after physical release until consumed)
   - `0x55` (`u8`): `oneshot_layer` (`0xFF` if none)
   - `0x56` (`u8`): `qmk_flags` (bit 0: `caps_word_active`, bit 1: `oneshot_locked`)
   - `0x57..0x5A` (`u32` LE): `default_layer_bitfield` (matches `profile_settings_v1`'s `u32` bitfield)
   - `0x5B`: reserved `0`
   - `0x6C..0x6F` (`u32` LE): `keymap_revision` (monotonic counter incremented on every `SEND_KEYMAP` or Vial keymap write)
   - `0x70..0x7B` (12 bytes = 96 bits): `tap_hold_pending_bitmap` (populated once Phase 2 tap-hold exists; `0` until then)
   - `0x7C..0x83` (8 bytes): reserved `0`
2. **Firmware-resolved key faces (`FW_UART_MSG_RESOLVED_KEYS = 6`):**
   - Do not duplicate `KC_TRANSPARENT` fallthrough, Key Overrides (`Shift+Bksp -> Del`), and Combos in Lua.
   - Whenever `(active_layer_bitfield, effective_mods | oneshot_mods, qmk_flags, keymap_revision)` changes, the firmware emits **`FW_UART_MSG_RESOLVED_KEYS = 6`** containing `{ keymap_revision: u32, n_keys: u8, entries[n_keys] }` where each entry is `{ resolved_word: u32, source_layer: u8 }` (up to `90 × 5 + ~8 ≈ 458` bytes, fitting inside `FW_HDLC_MAX_LEN = 512`).
   - *Rate-limiting note:* With home-row mods, resolved holds flip `effective_mods` during fast typing (~4 ms per frame at 921,600 baud). Coalesce/rate-limit `FW_UART_MSG_RESOLVED_KEYS` behind `MSG_UPDATE` so key-state updates are never delayed, and never expect a bridge ACK for type 6 so older bridges safely drop it.
   - *Display simplification:* Because firmware pushes resolved faces directly, `openflux-display` no longer needs `openfluxd` to duplicate keycodes via `keymap.json` (which only needs stylemaps/icons/labels, and edits made in `vial.rocks` on another machine without `openfluxd` appear on the keyboard screen immediately).

### Decision 6: Visual UI — Vial Raw HID (IF 5) + Native Studio Tabs

1. **Firmware exposes Vial Raw HID (`0xFF60:0x61`) on interface 5:**
   - Report VIA protocol version (e.g. `0x0009` / current VIA version), Vial protocol `0x00000006`, and an 8-byte `VIAL_KEYBOARD_UID`.
   - Unlocks **`vial-gui` and `vial.rocks`** immediately with **zero** GPL code inside OpenFlux Studio.
2. **Keep OpenFlux Studio (`host/studio`) native (`MIT OR Apache-2.0`):**
   - Extend `KeymapStylemapEditor.tsx` and `KeycodePickerModal.tsx` for dual-role keys (`0x6`), `OSM`/`OSL`/`Caps Word` (`0x7`), and Flux module `[[actions]]`, plus native tabs for Tap Dance, Combos, Key Overrides, and QMK Settings.
   - Studio writes native 32-bit keymaps over IF 1 (preserving actuation tables); `openfluxd` watches `keymap_revision` in `MSG_UPDATE` / `GET_RUNTIME_STATE` so external Vial edits sync back into Studio immediately.

### Compatibility & Versioning (`compat.toml`)
- `uart` `1.0` → `1.1` (additive `MSG_UPDATE` fields + unacknowledged message types `5` and `6`).
- `firmware-usb` `1.1` → `1.2` (IF 4 Report ID 3/4 + `wMaxPacketSize` bump + IF 5 Vial Raw HID).
- `keymap` `1.0` → `1.1` (new `0x6`/`0x7` keycode kinds and extended tables; **host must require firmware supporting `keymap >= 1.1` before writing those words**, since older firmware silently treats unknown kinds as no-ops).
- `display-ipc` `1.0` → `1.1` (new `key_state` fields + `resolved_keys` / `flux_action` events).

---

## 3. Alternatives Considered

### Alternative 1: Full Port to `vial-qmk` (`CUSTOM_MATRIX = yes`, ChibiOS)
- **How it would be done:** Create a `vial-qmk` board on RP2040 with `CUSTOM_MATRIX = yes`; wrap `scan_sensors()` + the Hall-effect math in `matrix_scan_custom()`; port LittleFS (`0x00F00000`) and constrain QMK wear-leveling below it; re-implement UART1 HDLC, USB IF 1 bulk commands, SoC reset GPIOs, and USB IF 4 (`0xFF00`) under ChibiOS USB; adapt Vial's single-EEPROM keymap model for ANSI/ISO/JIS hot-swap.
- **Pros:** Upstream QMK engine + Vial Raw HID work out of the box with no custom action state machine; standard QMK CLI compilation.
- **Cons:** Heavy ChibiOS port of every custom Flux hardware protocol; awkward fit with dynamic ANSI/ISO/JIS keyframes and per-key actuation tables; switches firmware to GPL-2.0.
- **Why not picked:** Rebuilding Flux's hardware/USB/UART/LittleFS plumbing in ChibiOS carries more risk and work than replacing only the post-scan action layer in the working Pico SDK firmware.

### Alternative 2: Port to ZMK + ZMK Studio (Zephyr RTOS, MIT + Apache-2.0)
- **How it would be done:** Write a Zephyr KSCAN driver for the 160-channel Hall-effect scanner; port USB/UART/LittleFS to Zephyr; embed `zmkfirmware/zmk-studio` (Apache-2.0) speaking Protobuf RPC.
- **Pros:** Permissive licenses (`MIT` + `Apache-2.0`), clean Protobuf RPC, solid hold-tap/combos.
- **Cons:** Behavior model is ZMK rather than QMK/Vial (`MT`/`LT` keycodes, Tap Dance, Key Overrides, QMK Settings), and requires porting all RP2040 drivers to Zephyr.
- **Why not picked:** High driver-porting churn for a wired RP2040 when QMK-style features and Vial compatibility are the target.

### Alternative 3: Compile a Trimmed Subset of QMK `quantum/` Into Pico SDK
- **How it would be done:** Shim QMK's `tmk_core` timer, EEPROM, matrix, and HID report hooks inside Pico SDK and compile upstream `action_tapping.c`, `process_combo.c`, `process_tap_dance.c`, `process_key_override.c`, and `process_caps_word.c` directly into `openflux_firmware`.
- **Pros:** Upstream QMK timing behavior without ChibiOS.
- **Cons:** Hard integration—QMK's `quantum/` is tightly coupled to `tmk_core` globals, 16-bit keycodes, and boolean matrices; forces GPL-2.0 on the firmware binary and complicates native 32-bit dual-role keys and depth-aware tap-hold.
- **Why not picked:** Using QMK's `quantum/` as a host-only test oracle gets high confidence on QMK timing without entangling the shipped firmware with `tmk_core` internals or GPL-2.0.

### Alternative 4: Embed `tcyeee/vialite` (GPL-2.0 React/TS) Inside OpenFlux Studio
- **How it would be done:** Vendor `tcyeee/vialite` into `host/studio`, swap WebHID for `openfluxd` IPC, and relicense Studio.
- **Pros:** Pre-built React UI for layers, dual-role keys, macros, tap dance, combos, and QMK settings.
- **Cons:** Splits key editing across two keyboard canvases inside one app (`vialite` vs. `KeymapStylemapEditor.tsx` stylemaps/actuation/`previewRects`); `vialite` is GPL-2.0-only, creating a license conflict with any Apache-2.0-only Rust crate bundled in the Tauri binary (`deny.toml`); younger solo project missing Key Overrides UI.
- **Why not picked:** Extending Studio's existing native components avoids split UX and license conflicts with comparable effort, while interface 5 Raw HID still gives external Vial compatibility.

### Alternative 5: External `vial.rocks` / `vial-gui` Only (No Studio QMK UI)
- **How it would be done:** Expose Vial Raw HID on firmware; use `vial.rocks` or `vial-gui` for all QMK editing and Studio only for actuation, themes, and modules.
- **Pros:** Zero QMK UI code in Studio.
- **Cons:** Two-app workflow; stock Vial cannot dynamically show Flux module `[[actions]]` or unify key bindings with per-key screen stylemaps/icons.
- **Why not picked as the sole UI:** Interface 5 gives us external Vial support anyway, while native Studio tabs give a single cohesive workflow.

### Alternative 6: Extend Existing `keymap_blob` + `KeymapStylemapEditor.tsx` With No Vial Protocol
- **How it would be done:** Add custom `MT`/`LT` tags to the binary blob and Studio UI with no Vial Raw HID interface.
- **Pros:** Minimal diff for basic mod-tap only.
- **Cons:** Leaves out interoperability with `vial-gui`, `vial.rocks`, and `.vil` sharing.
- **Why not picked:** `fw_vial.c` as a thin translation layer over the native 32-bit store gives Vial compatibility at modest cost.

---

## 4. Phased Delivery & Open Design Choices

### Phased Plan
1. **Phase 1 — Display State Truth (`uart 1.1` / `display-ipc 1.1`):**
   Populate `MSG_UPDATE` capability marker (`0x52`) and `effective_mods`, `oneshot_mods`, `oneshot_layer`, `qmk_flags`, `default_layer_bitfield`, `keymap_revision` honestly from today's engine (leaving `tap_hold_pending_bitmap` zero until Phase 2); fix `keys-renderer/script.lua` to use `keyboard/state` and full `active_layer_bitfield`.
2. **Phase 2 — QMK Action Engine (`fw_qmk_engine.c`) + Host Test Oracle:**
   - *2a:* Event queue with `(pressed, depth_um)`, native dual-role (`0x6`) and QMK special (`0x7`) words, layers (`MO`/`TG`/`TO`/`DF`/`TT`/`OSL`), `MT`/`LT` (`TAPPING_TERM`, `PERMISSIVE_HOLD`, `HOLD_ON_OTHER_KEY_PRESS`, `QUICK_TAP_TERM`), `OSM`/`OSL`, Caps Word, Macros, rapid-trigger guard on dual-role keys—validated against QMK's host test oracle on scripted boolean sequences plus firmware depth/rapid-trigger tests.
   - *2b:* Combos, Tap Dance, Key Overrides, `CHORDAL_HOLD`, `FLOW_TAP_TERM`, `RETRO_TAPPING`, `tap_hold_pending_bitmap`, and rate-limited `FW_UART_MSG_RESOLVED_KEYS = 6`.
3. **Phase 3 — Flux-Only Module Actions:**
   IF 4 Report ID 3/4 (`wMaxPacketSize` bump), `FW_UART_MSG_FLUX_ACTION = 5`, sticky action slots stored with the keymap, `module.toml` `[[actions]]`, and `openfluxd` / `openflux-display` `input/flux/action` bus publishing.
4. **Phase 4 — Vial Raw HID (IF 5) + Native Studio Tabs:**
   `fw_vial.c` translation layer with `OF_NATIVE` preservation sentinel, debounced LittleFS flush, `keymap_revision` sync, and native Studio tabs for Tap Dance, Combos, Key Overrides, QMK Settings, and Flux module actions.

### Open Choices to Settle Before Phases 2–4
1. **Tap-hold timing settings scope:** Global (`qmk_settings`, matching Vial directly) or per-profile overrides (alongside actuation and `default_layer_bitfield` in `profile_settings_v1`)?
2. **Vial unlock policy:** Implement Vial's two-key hold unlock flow, or report unlocked (consistent with `openfluxd`, which already allows `ENTER_BOOTLOADER` over IF 1 without a key combo)?
3. **Vial + magnetic keyframe hot-swap (three options):**
   - *Option A:* Unified `6×16` superset matrix in `vial.json` with Vial layout options (simple for Vial, but collapses separate ANSI/ISO/JIS keymaps into one).
   - *Option B:* Scope Vial Raw HID to the attached layout and trigger USB re-enumeration on magnetic swap so Vial refreshes `vial.json` (preserves separate keymaps, briefly drops `openfluxd` handles; `vial.rocks` reconnect behavior unverified).
   - *Option C:* Single `6×16` superset matrix in `vial.json` backed by the **attached layout's** separate keymap (keeps separate ANSI/ISO/JIS keymaps without USB re-enumeration; Vial geometry stays usable, though Vial still needs a reopen/refresh to see swapped-layout bindings).
