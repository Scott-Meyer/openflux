#ifndef FW_REST_RUNTIME_H
#define FW_REST_RUNTIME_H

#include "fw_rest.h"

typedef struct {
    int32_t initial_delta;
    uint32_t since_ms, raw_sum;
    uint16_t samples;
    bool reacquire;
} fw_rest_sensor_watch_t;

typedef struct {
    uint8_t layout;
    bool inhibit_all_new, inhibit[6][16];
    uint8_t key[6][16]; /* FW_REST_CLEAR/HELD/UNCERTAIN */
    bool thermal_shift, no_match, finished;
    uint16_t tracked_keys, reacquiring, thermal_generation;
    uint32_t start_ms, expire_from_ms, last_adoption_ms;
    bool suppress_window;
    uint16_t suppress_mask[6];
    fw_rest_sensor_watch_t watch[FW_REST_CHANNELS];
    bool quiet[6][16];
    uint32_t quiet_from_ms[6][16], suppress_key_from_ms[6][16];
    uint32_t last_pressed_ms, last_idle_sample_ms;
    uint8_t idle_next, idle_count;
    bool rest_saved;
    uint16_t idle[9][FW_NUM_SENSORS];
} fw_rest_session_t;

/* Begin on each completed attached-frame acquisition, not every time a key is pressed. */
void fw_rest_begin(fw_rest_session_t *s, uint8_t layout, const fw_rest_guard_t *guard, bool bridge_connected,
                   bool qc_mode, uint16_t thermal_generation, uint32_t now_ms);

/* After actuation and thermal correction for this sweep. The input deltas are from before row conditioning.
 * `proposed` is the pending-down state, not the transition counter. `thermal_busy` includes either ongoing
 * compensation or its post-adjustment hold, and `thermal_generation` increments on an adjustment.
 * inhibit/inhibit_all_new apply to new presses on the next sweep, not already held keys. */
void fw_rest_recover(fw_rest_session_t *s, uint32_t now_ms, const uint16_t raw[FW_NUM_SENSORS],
                     const int32_t delta[FW_NUM_SENSORS], const uint8_t confirmed[6][16], const uint8_t proposed[6][16],
                     bool thermal_busy, uint16_t thermal_generation, uint16_t baseline[FW_NUM_SENSORS]);

/* Called after recovery. May refresh baseline from an idle median; returns true only when to write a new
 * REST record for this layout/feel (first 112 refreshed baseline values). Never performs flash I/O. */
bool fw_rest_idle(fw_rest_session_t *s, uint32_t now_ms, bool any_pressed, const uint16_t raw[FW_NUM_SENSORS],
                  uint16_t baseline[FW_NUM_SENSORS]);

#endif
