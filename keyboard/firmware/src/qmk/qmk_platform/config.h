#pragma once

/* OpenFlux QMK + Vial configuration (compiled inside Pico SDK / host test shell). */

#define MATRIX_ROWS 6
#define MATRIX_COLS 16

/* Full Vial + QMK feature set */
#define RAW_ENABLE
#define VIA_ENABLE
#define VIAL_ENABLE
#define DYNAMIC_KEYMAP_ENABLE
#define QMK_SETTINGS
#define NKRO_ENABLE
#define NKRO_DEFAULT_ON true
#define EXTRAKEY_ENABLE
#define SEND_STRING_ENABLE
#define CAPS_WORD_ENABLE
#define CHORDAL_HOLD
#define TAP_DANCE_ENABLE
#define VIAL_TAP_DANCE_ENABLE
#define COMBO_ENABLE
#define VIAL_COMBO_ENABLE
#define KEY_OVERRIDE_ENABLE
#define VIAL_KEY_OVERRIDE_ENABLE
#define REPEAT_KEY_ENABLE
#define VIAL_ALT_REPEAT_KEY_ENABLE

/* Consistent with IF 1 ENTER_BOOTLOADER: no two-key unlock hold required. */
#define VIAL_INSECURE

/* Serve dynamic XZ vial.json + cache-busted UID from RAM/LittleFS in vial.c */
#define FW_QMK_DYNAMIC_VIAL_DEF

/* Default 8-byte Vial UID (low 4 bytes get mixed with dynamic vial.json hash when overridden). */
#define VIAL_KEYBOARD_UID {0x4F, 0x46, 0x4C, 0x58, 0x00, 0x00, 0x00, 0x01}

/* EEPROM budget: 8 KiB flat buffer backed by per-layout LittleFS files. */
#define EEPROM_CUSTOM
#define EEPROM_SIZE                   8192
#define DYNAMIC_KEYMAP_LAYER_COUNT    8
#define DYNAMIC_KEYMAP_MACRO_COUNT    32
#define VIAL_TAP_DANCE_ENTRIES        32
#define VIAL_COMBO_ENTRIES            32
#define VIAL_KEY_OVERRIDE_ENTRIES     32
#define VIAL_ALT_REPEAT_KEY_ENTRIES   16

#define RAW_EPSIZE       32
#define VIAL_RAW_EPSIZE  32
#define KEYBOARD_EPSIZE  8
#define SHARED_EPSIZE    32

/* Standard USB VID/PID (matches fw_protocol.h) */
#define VENDOR_ID  0x3761
#define PRODUCT_ID 0x4015
#define DEVICE_VER 0x0101

/* Disable unused QMK console/debug/bootmagic/mousekey/command subsystems */
#define NO_DEBUG
#define NO_PRINT
#define NO_ACTION_MACRO
#define NO_ACTION_FUNCTION

/* Tapping / home-row mod defaults (runtime-tunable via QMK Settings in Vial) */
#define TAP_CODE_DELAY 0
#define TAPPING_TERM 200
#define QUICK_TAP_TERM 120
#define FLOW_TAP_TERM 150
#define PERMISSIVE_HOLD
#define TAPPING_TERM_PER_KEY
#define QUICK_TAP_TERM_PER_KEY
#define PERMISSIVE_HOLD_PER_KEY
#define HOLD_ON_OTHER_KEY_PRESS_PER_KEY
#define RETRO_TAPPING_PER_KEY

/* Prevent QMK from pulling AVR/ChibiOS headers */
#define NO_USB_STARTUP_CHECK
