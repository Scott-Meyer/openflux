#ifndef FW_PRESS_GUARD_H
#define FW_PRESS_GUARD_H

#include "fw_rapid.h"

/* Apply temporary calibration guards to the decision signal only. Row motion uses the original flux. A key held
 * through a guarded interval keeps its release protection until it is up. */
static inline uint16_t fw_guard_key_signal(uint16_t flux, uint16_t weight_q8, bool down, bool rapid, uint16_t peak,
                                           bool require_strong_signal, uint8_t *protected_held, bool mask_presses) {
    uint32_t weight = weight_q8 ? weight_q8 : 256U;
    if (!down) {
        *protected_held = 0;
        if (mask_presses || (require_strong_signal && flux < ((1000U * weight) >> 8)))
            return 0;
    } else if (require_strong_signal || *protected_held) {
        if (flux < ((600U * weight) >> 8))
            return 0;
        if (rapid && peak > flux)
            return peak;
    }
    return flux;
}

/* Return a candidate, not a committed key. Ignore at most six prospective press samples disturbed by adjacent
 * motion in one uninterrupted above-threshold excursion. A sustained spatial guard can still veto the rest. */
static inline bool fw_fixed_candidate(bool down, int32_t flux, int32_t act, int32_t release, int32_t neighbor_delta,
                                      const fw_key_motion_t *motion, uint8_t *neighbor_vetoes) {
    if (down)
        return flux > release || (motion->inhibit_release && 2 * flux > release);
    if (flux <= act) {
        *neighbor_vetoes = 0;
        return false;
    }
    if (*neighbor_vetoes < 6 && neighbor_delta > 15) {
        ++*neighbor_vetoes;
        return false;
    }
    return !motion->inhibit_press;
}

#endif /* FW_PRESS_GUARD_H */
