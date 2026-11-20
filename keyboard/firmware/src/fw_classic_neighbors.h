#ifndef FW_CLASSIC_NEIGHBORS_H
#define FW_CLASSIC_NEIGHBORS_H

#include <stdint.h>

/* Neighboring-key correction for the model-free key tracker, used when press refinement is not active. */

/* One instance per physical row. Zero-initialize at boot. */
typedef struct {
    uint8_t below_release[16];
} fw_classic_neighbors_t;

/* Correct a complete row before updating any key decisions for this scan. field contains
 * keys+1 signed, conditioned sensor counts; flux contains keys provisional travel values
 * (including the switch-feel gain) and is updated in place. down is the previous scan's
 * confirmed state, not a pending proposal. The two threshold arrays are the *resolved*,
 * position-scaled actuation and hysteresis depths; pass both NULL for an uninitialized
 * keymap (unscaled defaults). This row can contain at most 16 keys. */
void fw_classic_neighbors(fw_classic_neighbors_t *state, unsigned layout, unsigned row, unsigned keys,
                          const int32_t field[], int32_t flux[], const uint8_t down[], const uint16_t actuation[],
                          const uint16_t hysteresis[]);

#endif
