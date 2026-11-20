#ifndef FW_THERMAL_H
#define FW_THERMAL_H

#include <stdbool.h>
#include <stdint.h>

/* Runtime-only drift history. Zero on power-up; retain across sweeps. */
typedef struct {
    uint16_t flux;
    uint8_t down;    /* confirmed now */
    uint8_t pending; /* proposed down (same as down after a committed rapid transition) */
    uint8_t masked;  /* the boot/held guard masks this key */
} fw_thermal_key_t;

typedef struct {
    uint16_t sum[12];
    uint16_t history[2][4][12]; /* 16-sample totals of the dedicated reference channels */
    int16_t frames[2][4][112];  /* last ADC sweep at each history boundary */
    uint64_t clock_ms, history_ms[2];
    uint8_t next[2], windows, initialized, module_mode, evidence, layout, reference_count;
    uint8_t reference_channel[12];
    int16_t inverse_center[112];
    int32_t gain_q10;
    int32_t select[112]; /* Scratch shared by median estimates, off the MCU stack. */
    uint64_t disturbance_until, chord_until, last_drift, last_chord, last_refresh;
    bool latched_drift, sustained_drift, deferred_held, moving;
    bool require_strong, refreshed;
    uint8_t protect_held[6][16];
    uint16_t previous_flux[6][16], window_flux[6][16];
    uint16_t collected[112]; /* sum of at most sixteen 12-bit readings */
    uint8_t collected_count;
    uint16_t refresh_count;
} fw_thermal_state_t;

/* Call after a new resting acquisition, including any held-at-boot substitutions. */
void fw_thermal_acquired(fw_thermal_state_t *s, const uint16_t baseline[160]);

/* Evaluate after this sweep's decisions, before the idle/held guard. Baseline changes and
 * guards apply on the next sweep. Return true when the key-depth tracker and rapid
 * travel histories must be restarted. module_fields: accessory modules are influencing
 * the reference sensors; use the quiet-key correlation check rather than spare channels. */
bool fw_thermal_after_scan(fw_thermal_state_t *s, uint8_t layout, uint32_t ms, const uint16_t raw[160],
                           uint16_t baseline[160], const fw_thermal_key_t keys[6][16], bool module_fields);
#endif
