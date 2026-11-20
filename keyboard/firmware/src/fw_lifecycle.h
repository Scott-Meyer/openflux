#ifndef FW_LIFECYCLE_H
#define FW_LIFECYCLE_H

#include "fw_keyframe.h"
#include "fw_rest_runtime.h"
#include "fw_thermal.h"

struct fw_runtime;

typedef struct {
    fw_keyframe_detector_t detector;
    fw_rest_session_t rest;
    fw_thermal_state_t thermal;
    /* Baseline classification runs between scan frames. Share its scratch storage with
     * the next frame's observations instead of placing it on the small MCU stack. */
    union {
        fw_rest_guard_t calibration_guard;
        fw_thermal_key_t observed[6][16];
    };
    uint8_t confirmed[6][16], proposed[6][16];
    bool calibrated;
    uint8_t attach_phase; /* 0 absent, 1 detecting, 2 settling, 3 attached, 4 recovering */
} fw_lifecycle_t;

/* A change in frame identity releases every key. True requests a new measurement; load
 * the REST file for the reported layout and feel before passing in that measurement. */
bool fw_runtime_poll_frame(struct fw_runtime *rt, const uint16_t raw_adc[FW_NUM_SENSORS], uint32_t now_ms);
void fw_runtime_calibrate(struct fw_runtime *rt, const uint16_t mean[FW_NUM_SENSORS],
                          const uint8_t unstable[FW_REST_CHANNELS], uint32_t now_ms, bool bridge_connected);

/* Apply limits from earlier scans to the decision signal only. Retain the original flux
 * for motion tracking and the end-of-scan observation below. */
uint16_t fw_runtime_guard_flux(struct fw_runtime *rt, unsigned row, unsigned col, uint16_t flux, uint16_t gain_q8,
                               bool down, bool rapid, uint16_t peak);

/* After each key decision, record the unmasked signal and both states. A fixed key can be
 * pending down before confirmation; rapid keys publish their committed state here. */
void fw_runtime_observe_key(struct fw_runtime *rt, unsigned row, unsigned col, uint16_t tracked_flux, bool down,
                            bool proposed_down);

/* Finish an attached scan after evaluating keys. Returns true only when rest_ref_adc is
 * ready to persist under the current layout/feel. Does not perform any flash I/O. */
bool fw_runtime_finish_scan(struct fw_runtime *rt, uint32_t now_ms);

#endif /* FW_LIFECYCLE_H */
