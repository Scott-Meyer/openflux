#include "fw_thermal.h"

#include <assert.h>

/* A user holds Esc while the board warms, then releases it. Do not learn its travel as the
 * new rest position. Include the same uniform field change at the unoccupied reference slots. */
void test_fw_thermal(void) {
    fw_thermal_state_t thermal = {0};
    fw_thermal_key_t keys[6][16] = {0};
    uint16_t raw[160], baseline[160];
    for (unsigned i = 0; i < 160; i++)
        raw[i] = baseline[i] = 2304;
    fw_thermal_acquired(&thermal, baseline);
    unsigned guarded = 0, protected_refreshes = 0, quiet_refreshes = 0;
    for (unsigned sweep = 0; sweep < 1600; sweep++) {
        if (sweep == 50) {
            keys[0][0] = (fw_thermal_key_t){.flux = 1000, .down = 1, .pending = 1};
            raw[6] += 600;
            raw[13] -= 600;
        }
        if (sweep == 150) {
            for (unsigned i = 0; i < 112; i++)
                raw[i] += 46;
        }
        if (sweep == 800) {
            keys[0][0] = (fw_thermal_key_t){0};
            raw[6] -= 600;
            raw[13] += 600;
        }
        bool refresh = fw_thermal_after_scan(&thermal, 1, 1000 + 2 * sweep, raw, baseline, keys, false);
        if (sweep < 150) {
            assert(!refresh && !thermal.require_strong && baseline[6] == 2304 && baseline[13] == 2304);
        } else if (sweep < 800) {
            guarded += thermal.require_strong;
            if (refresh) {
                assert(thermal.protect_held[0][0]);
                assert(baseline[7] == 2350 && raw[6] - baseline[6] >= 600 && baseline[13] - raw[13] >= 500);
                protected_refreshes++;
            }
        } else if (refresh) {
            assert(!thermal.protect_held[0][0]);
            quiet_refreshes++;
        }
    }
    assert(guarded && protected_refreshes && quiet_refreshes && !thermal.require_strong);
    for (unsigned i = 0; i < 112; i++)
        assert(baseline[i] == 2350);

    /* An old timeout cannot reappear when a continuously powered board's millisecond
     * counter crosses the signed boundary or wraps. There is no motion to suppress. */
    const uint32_t long_idle[] = {0x80000000U, 0xffffffe0U, 4096U};
    for (unsigned i = 0; i < 3; i++)
        for (uint32_t sweep = 0; sweep < 32; sweep++) {
            assert(!fw_thermal_after_scan(&thermal, 1, long_idle[i] + sweep, raw, baseline, keys, false));
            assert(!thermal.require_strong && !thermal.moving);
        }
}
