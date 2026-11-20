#include "fw_lifecycle.h"
#include "fw_scan_hot.h"
#include "fw_press_guard.h"
#include "fw_scanner.h"

#include <string.h>

bool fw_runtime_poll_frame(fw_runtime_t *rt, const uint16_t raw_adc[FW_NUM_SENSORS], uint32_t now_ms) {
    fw_lifecycle_t *l = &rt->lifecycle;
    /* Identity and the published phase both update on the 100 ms detector tick. */
    uint32_t last_poll = l->detector.last_poll_ms;
    l->detector.stress_frame = rt->stress_test_enabled != 0;
    const fw_keyframe_result_t frame = fw_keyframe_poll(&l->detector, raw_adc, now_ms);
    if (last_poll && l->detector.last_poll_ms == last_poll)
        return false;
    bool changed = frame.changed || rt->keyframe_attached != frame.attached || rt->keyframe_layout != frame.layout ||
                   rt->switch_feel != frame.feel;
    if (changed)
        fw_runtime_set_keyframe(rt, frame.attached, frame.layout, frame.feel);
    l->attach_phase = frame.attached && l->calibrated && l->rest.inhibit_all_new ? 4U : frame.stage;
    return changed && frame.attached;
}

void fw_runtime_calibrate(fw_runtime_t *rt, const uint16_t mean[FW_NUM_SENSORS],
                          const uint8_t unstable[FW_REST_CHANNELS], uint32_t now_ms, bool bridge_connected) {
    fw_lifecycle_t *l = &rt->lifecycle;
    memmove(rt->baseline_adc, mean, sizeof rt->baseline_adc);
    if (!rt->keyframe_attached) {
        l->calibrated = false;
        return;
    }
    fw_rest_apply(rt->keyframe_layout, rt->baseline_adc, unstable, rt->rest_ref_valid ? rt->rest_ref_adc : NULL,
                  &l->calibration_guard);
    fw_thermal_acquired(&l->thermal, rt->baseline_adc);
    fw_rest_begin(&l->rest, rt->keyframe_layout, &l->calibration_guard, bridge_connected, rt->qc_mode != 0,
                  l->thermal.refresh_count, now_ms);
    memset(l->observed, 0, sizeof l->observed);
    fw_runtime_reset_tracking(rt);
    l->calibrated = true;
}

uint16_t FW_SCAN_CODE(fw_runtime_guard_flux)(fw_runtime_t *rt, unsigned row, unsigned col, uint16_t flux,
                                             uint16_t gain_q8, bool down, bool rapid, uint16_t peak) {
    fw_lifecycle_t *l = &rt->lifecycle;
    if (!l->calibrated)
        return flux;
    return fw_guard_key_signal(flux, gain_q8, down, rapid, peak, l->thermal.require_strong,
                               &l->thermal.protect_held[row][col],
                               l->rest.inhibit_all_new || l->rest.inhibit[row][col]);
}

void FW_SCAN_CODE(fw_runtime_observe_key)(fw_runtime_t *rt, unsigned row, unsigned col, uint16_t tracked_flux,
                                          bool down, bool proposed_down) {
    rt->lifecycle.observed[row][col] = (fw_thermal_key_t){.flux = tracked_flux, .down = down, .pending = proposed_down};
}

bool fw_runtime_finish_scan(fw_runtime_t *rt, uint32_t now_ms) {
    fw_lifecycle_t *l = &rt->lifecycle;
    if (!l->calibrated || !rt->keyframe_attached)
        return false;

    bool any_pressed = false;
    for (unsigned row = 0; row < 6; row++) {
        for (unsigned col = 0; col < 16; col++) {
            fw_thermal_key_t *key = &l->observed[row][col];
            key->masked = l->rest.inhibit[row][col];
            l->confirmed[row][col] = key->down;
            l->proposed[row][col] = key->pending;
            any_pressed |= key->down != 0;
        }
    }
    /* Accessory module fields are not detected; pass false. */
    if (fw_thermal_after_scan(&l->thermal, rt->keyframe_layout, now_ms, rt->raw_adc, rt->baseline_adc, l->observed,
                              false))
        fw_runtime_reset_tracking(rt);
    fw_rest_recover(&l->rest, now_ms, rt->raw_adc, rt->sensor_scaled, l->confirmed, l->proposed,
                    l->thermal.require_strong || l->thermal.moving, l->thermal.refresh_count, rt->baseline_adc);
    /* A shorter or replaced keymap must not retain observations for keys it no longer evaluates. */
    memset(l->observed, 0, sizeof l->observed);
    if (!fw_rest_idle(&l->rest, now_ms, any_pressed, rt->raw_adc, rt->baseline_adc))
        return false;

    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        rt->rest_ref_adc[i] = (int16_t)rt->baseline_adc[i];
    rt->rest_ref_valid = 1;
    return true;
}
