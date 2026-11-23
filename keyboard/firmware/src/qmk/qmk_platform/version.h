#pragma once

/*
 * Pinned EEPROM schema date for VIA/Vial magic validation.
 * Bump ONLY when EEPROM sizes/layout in config.h change, so ordinary firmware
 * updates never wipe the user's saved keymaps, combos, tap dances, or macros.
 */
#define QMK_BUILDDATE "2026-11-23-00:00:00"
#define QMK_VERSION   "openflux-vial"
#define BUILD_ID      0x004F4601UL
