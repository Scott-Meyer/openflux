#include "fw_rapid.h"
#include "fw_scan_hot.h"

static int32_t magnitude(int32_t x) {
    return x < 0 ? -x : x;
}

static uint8_t guard_window(uint8_t left, bool renew) {
    return renew ? 30U : (left ? left - 1U : 0U);
}

void FW_SCAN_CODE(fw_update_row_motion)(fw_key_motion_t row[], const uint16_t flux[], unsigned n) {
    for (unsigned i = 0; i < n; ++i) {
        fw_key_motion_t *m = &row[i];
        m->delta = (int32_t)flux[i] - m->previous_flux;
        m->previous_flux = flux[i];
        m->velocity += m->delta - (m->velocity >> 3);
    }
    for (unsigned i = 0; i < n; ++i) {
        fw_key_motion_t *m = &row[i];
        int32_t l = i ? row[i - 1].velocity : 0;
        int32_t r = i + 1 < n ? row[i + 1].velocity : 0;
        int32_t v = m->velocity;
        /* Similar movement on both sides can disturb a key between them. Strong independent
         * movement overrides a newly raised guard before it can mask an actual stroke. */
        bool between = 2 * magnitude(l) > magnitude(v) && 2 * magnitude(r) > magnitude(v);
        m->inhibit_press = guard_window(m->inhibit_press, between && l < -40 && r < -40);
        m->inhibit_release = guard_window(m->inhibit_release, between && l > 40 && r > 40);
        int32_t largest = magnitude(l) > magnitude(r) ? magnitude(l) : magnitude(r);
        if (v > 40 && v > 2 * largest && m->inhibit_press > 25)
            m->inhibit_press = 0;
        if (v < -40 && -v > 2 * largest && m->inhibit_release > 25)
            m->inhibit_release = 0;
    }
}

int32_t FW_SCAN_CODE(fw_neighbor_delta)(const fw_key_motion_t row[], unsigned col, unsigned n) {
    int32_t l = col ? magnitude(row[col - 1].delta) : 0;
    int32_t r = col + 1 < n ? magnitude(row[col + 1].delta) : 0;
    return l > r ? l : r;
}

void fw_rapid_rearm(fw_rapid_state_t *s) {
    s->peak = s->trough = 0;
    s->reversing = s->pending_down = s->pending_scans = 0;
}

static void track_independent_motion(fw_rapid_state_t *s, int32_t step, int32_t neighbor) {
    bool independent = step > 20 && step > 2 * neighbor;
    int32_t previous = s->last_independent_step;
    if (previous > 40 && 4 * magnitude(previous + step) < previous) {
        s->independent_scans = 0;
    } else if (independent) {
        s->independent_scans = 24;
    } else if (s->independent_scans) {
        --s->independent_scans;
    }
    s->last_independent_step = independent ? step : 0;
}

static bool neighbor_moving(fw_rapid_state_t *s, int32_t neighbor) {
    if (neighbor > 20) {
        s->neighbor_scans = 2;
        return true;
    }
    if (!s->neighbor_scans)
        return false;
    --s->neighbor_scans;
    return true;
}

bool FW_SCAN_CODE(fw_rapid_down)(fw_rapid_state_t *s, bool down, const fw_rapid_input_t *in) {
    int32_t f = in->flux, a = in->actuation;
    int32_t rearm = a - in->hysteresis;
    if (rearm < 120)
        rearm = 120;
    track_independent_motion(s, down ? -in->motion->delta : in->motion->delta, in->neighbor_delta);
    if ((!down && in->motion->inhibit_press) || (down && in->motion->inhibit_release && f >= rearm))
        return down;

    int32_t reversal = in->sensitivity;
    if (in->row_active > 1 && !s->independent_scans)
        reversal *= 2;
    bool want = down, first = false, bottom_release = false;
    if (down) {
        if (f > s->peak)
            s->peak = (uint16_t)f;
        want = f >= (int32_t)s->peak - reversal && (f > a || f >= rearm);
        /* Above bottom travel the peak is capped. A release crossing back through that
         * cap uses nominal sensitivity even when other keys in the row are held. */
        int32_t bottom = (1162 * (in->weight_q8 ? in->weight_q8 : 256)) >> 8;
        if (f >= bottom) {
            s->peak = (uint16_t)bottom;
            want = true;
        } else if (s->peak == bottom && f < bottom - in->sensitivity) {
            bottom_release = true;
            want = false;
        }
    } else if (!s->reversing && f > a) {
        s->reversing = true;
        s->trough = (uint16_t)f;
        want = first = true;
    } else {
        if (s->reversing) {
            if (f < s->trough)
                s->trough = (uint16_t)f;
            want = f > (int32_t)s->trough + reversal && (f > a || f >= rearm);
        }
        if (f <= a && f < rearm) {
            s->reversing = false;
            s->peak = s->trough = 0;
        }
    }

    if (!first && !(bottom_release && s->independent_scans) && f >= rearm && neighbor_moving(s, in->neighbor_delta)) {
        want = down;
        if (in->neighbor_delta > 3 * magnitude(in->motion->delta))
            s->peak = s->trough = (uint16_t)f;
    }
    if (want == down) {
        s->pending_down = down;
        s->pending_scans = 0;
        return down;
    }
    if (!first) {
        if (s->pending_down != want) {
            s->pending_down = want;
            s->pending_scans = 1;
            return down;
        }
        if (s->pending_scans < (bottom_release ? 2 : 6)) {
            ++s->pending_scans;
            return down;
        }
    }
    if (want)
        s->peak = (uint16_t)f;
    else
        s->trough = (uint16_t)f;
    return want;
}
