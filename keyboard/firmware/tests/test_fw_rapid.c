#include "fw_rapid.h"

#include <assert.h>
#include <stddef.h>

/* One rapid key between two neighbors. Each row of a trajectory holds the three
 * positions for the indicated sequence of scans; each character is one key report. */
typedef struct {
    uint16_t left, key, right;
    const char *expected;
} rapid_hold_t;

static void check_trajectory(const rapid_hold_t *holds, size_t n) {
    fw_rapid_state_t key = {0};
    fw_key_motion_t row[3] = {0};
    bool down = false;
    for (size_t i = 0; i < n; i++) {
        const rapid_hold_t *h = &holds[i];
        uint16_t values[] = {h->left, h->key, h->right};
        for (const char *report = h->expected; *report; report++) {
            fw_update_row_motion(row, values, 3);
            fw_rapid_input_t in = {
                .flux = h->key,
                .actuation = 328,
                .hysteresis = 248,
                .sensitivity = 254,
                .weight_q8 = 256,
                .row_active = down,
                .motion = &row[1],
                .neighbor_delta = fw_neighbor_delta(row, 1, 3),
            };
            down = fw_rapid_down(&key, down, &in);
            assert(down == (*report == '1'));
        }
    }
}

void test_fw_rapid(void) {
    /* A clean stroke fires immediately. A partial release and retrigger must
     * cross (not just reach) the sensitivity distance from the peak/trough. */
    const rapid_hold_t partial[] = {
        {0, 400, 0, "1"},       {0, 700, 0, "1"},        {0, 446, 0, "11111111"},
        {0, 440, 0, "1111110"}, {0, 694, 0, "00000000"}, {0, 695, 0, "0000001"},
    };
    check_trajectory(partial, sizeof(partial) / sizeof(*partial));

    const rapid_hold_t reset[] = {
        {0, 400, 0, "1"},
        {0, 0, 0, "11111100"},
        {0, 328, 0, "0"},
        {0, 329, 0, "1"},
    };
    check_trajectory(reset, sizeof(reset) / sizeof(*reset));

    /* Saturating the sensor should not require travelling all the way back from
     * that reported overtravel to register a release. */
    const rapid_hold_t bottom[] = {
        {0, 400, 0, "1"},
        {0, 1800, 0, "1"},
        {0, 910, 0, "11111"},
        {0, 900, 0, "11000"},
    };
    check_trajectory(bottom, sizeof(bottom) / sizeof(*bottom));

    /* Two adjacent releases can move the key between them across actuation. A
     * small rise is suppressed; the same key can still take a deliberate stroke. */
    const rapid_hold_t neighbors[] = {
        {0, 0, 0, "0"},
        {600, 320, 600, "00000000"},
        {0, 330, 0, "000"},
        {0, 950, 0, "111"},
    };
    check_trajectory(neighbors, sizeof(neighbors) / sizeof(*neighbors));
}
