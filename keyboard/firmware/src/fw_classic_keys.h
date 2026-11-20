#ifndef FW_CLASSIC_KEYS_H
#define FW_CLASSIC_KEYS_H

#include <stdbool.h>
#include <stdint.h>

/* Model-free key tracker, used when press refinement is not active: fixed and rapid-trigger key decisions
 * from each row's corrected travel. The refinement path uses fw_rapid.h instead. */

/* One instance per physical row (up to 16 keys). Signal quantities are calibrated flux counts. */
typedef struct {
    uint16_t actuation[16], hysteresis[16]; /* Position-scaled depths, exposed to neighboring-key correction. */
    uint16_t sensitivity[16];               /* Unscaled rapid reversal distance. */
    uint8_t rapid[16];
} fw_classic_settings_t;

typedef struct {
    int32_t peak, trough;
    uint8_t armed, proposed, scans;
} fw_classic_reversal_t;

typedef struct {
    uint8_t down[16], pending[16]; /* Confirmed and proposed key-down state; rapid publishes committed edges only. */
    uint8_t scans[16];
    fw_classic_reversal_t reversal[16];
} fw_classic_keys_t;

/* Start with zeroed key state. Decode each slot on layout/keymap/mode load, even if its word is zero.
 * If the section is absent, pass has_table=false for unscaled fixed defaults. A mode change clears
 * reversal history but preserves down. Fixed loads also cancel pending fixed confirmation;
 * rapid loads retain it until a rapid edge. For a fresh frame with no held keys use zeroed state. */
void fw_classic_key_load(fw_classic_settings_t *settings, fw_classic_keys_t *keys, unsigned layout, unsigned row,
                         unsigned col, bool has_table, uint32_t word);

/* Apply a further gain pass to already-resolved thresholds, without resetting state. */
void fw_classic_key_weight(fw_classic_settings_t *settings, unsigned layout, unsigned row, unsigned col);

/* All rows must see the same held count from before this scan; row occupancy is snapshotted inside.
 * Update only after computing the entire row's corrected flux using the previous confirmed state. */
void fw_classic_keys_step(fw_classic_keys_t *keys, const fw_classic_settings_t *settings, unsigned row, unsigned n,
                          unsigned held_before, const int32_t flux[]);

#endif
