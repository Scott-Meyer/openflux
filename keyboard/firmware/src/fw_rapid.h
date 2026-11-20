#ifndef FW_RAPID_H
#define FW_RAPID_H

#include <stdbool.h>
#include <stdint.h>

/* Key decisions for the press-refinement path. They take one refined travel value per key plus row motion
 * hints, with per-key guard windows. The model-free tracker in fw_classic_keys.h works on a whole row's
 * travel and occupancy instead, so the two state machines are kept separate. */

/* All key-signal quantities are calibrated flux counts. Zero initialization is at rest. */
typedef struct {
    int32_t velocity;
    int32_t delta;
    uint16_t previous_flux;
    uint8_t inhibit_press, inhibit_release;
} fw_key_motion_t;

/* Update one complete row (all logical keys, not just rapid keys) with the unmodified
 * tracked signals for this scan, before using its movement hints to evaluate any key. */
void fw_update_row_motion(fw_key_motion_t row[], const uint16_t flux[], unsigned n);

/* Positive magnitude of the largest immediate neighbor's change since the previous scan. */
int32_t fw_neighbor_delta(const fw_key_motion_t row[], unsigned col, unsigned n);

typedef struct {
    int32_t last_independent_step;
    uint16_t peak, trough;
    uint8_t independent_scans, neighbor_scans;
    uint8_t reversing, pending_down, pending_scans;
} fw_rapid_state_t;

typedef struct {
    uint16_t flux;        /* Decision signal after any startup/held-key guards. */
    uint16_t actuation;   /* Position-weighted initial press threshold. */
    uint16_t hysteresis;  /* Position-weighted distance from actuation to reset. */
    uint16_t sensitivity; /* Unweighted rise/fall distance for partial retriggers. */
    uint16_t weight_q8;   /* Position gain, or zero for unity. */
    uint8_t row_active;   /* Keys in this row held or pending a fixed-mode press before this scan. */
    const fw_key_motion_t *motion;
    int32_t neighbor_delta;
} fw_rapid_input_t;

/* Evaluate one rapid key. On keymap/mode reload, clear travel and pending state with
 * fw_rapid_rearm; keyframe removal resets the entire state, including movement history. */
bool fw_rapid_down(fw_rapid_state_t *s, bool down, const fw_rapid_input_t *in);
void fw_rapid_rearm(fw_rapid_state_t *s);

#endif /* FW_RAPID_H */
