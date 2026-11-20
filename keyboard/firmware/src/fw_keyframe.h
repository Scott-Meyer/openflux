#ifndef FW_KEYFRAME_H
#define FW_KEYFRAME_H

#include <stdbool.h>
#include <stdint.h>

/* The normal sensor path recognizes two polarities for layout and switch feel.
 * A detached frame reports layout and feel zero; startup is detached. */
typedef struct {
    uint8_t attached, layout, feel;
    uint8_t stage; /* 0 absent, 1 field detected, 2 IDs settling, 3 attached */
    bool changed, calibrate;
} fw_keyframe_result_t;

typedef struct {
    uint16_t history[2][30];
    uint16_t previous[112];
    uint32_t last_poll_ms;
    uint8_t next, samples, stable_id[2];
    uint8_t stable_scans, wait_scans, missing_scans;
    bool stress_frame; /* Explicit test mode may supply an ANSI frame when no magnetic field is present. */
    fw_keyframe_result_t frame;
} fw_keyframe_detector_t;

/* Zero-initialize on power-up, not on detach. Call after each sensor sweep with boot-relative
 * milliseconds. Only samples separated by >=100 ms enter the detector. */
fw_keyframe_result_t fw_keyframe_poll(fw_keyframe_detector_t *d, const uint16_t raw[112], uint32_t now_ms);

#endif
