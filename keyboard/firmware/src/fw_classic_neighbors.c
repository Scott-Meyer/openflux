#include "fw_classic_neighbors.h"
#include "fw_scan_hot.h"

#include <string.h>

/* Sensor channels measure the field at the boundaries between keys. A run of
 * identical inferred travel is ambiguous without a local drop in that field. */
static void reject_plateaus(unsigned keys, const int32_t field[], int32_t flux[], const uint8_t down[]) {
    for (unsigned start = 0; start < keys;) {
        unsigned end = start + 1;
        while (end < keys && flux[end] == flux[start])
            ++end;
        unsigned length = end - start;
        if (length >= 3 || (length == 2 && end < keys)) {
            int limit = length == 2 ? 560 : 800;
            /* The last key has no following key with which to compare its field. */
            for (unsigned i = start; i < end && i + 1 < keys; ++i) {
                int32_t rise = (int32_t)((uint32_t)field[i + 1] - (uint32_t)field[i]);
                if (rise >= -79 && flux[i] < limit && !down[i])
                    flux[i] = 0;
            }
        }
        start = end;
    }
}

static uint32_t magnitude(int32_t x) {
    return x < 0 ? 0U - (uint32_t)x : (uint32_t)x;
}

/* Correct apparent center travel that two active neighbors can explain.
 * Snapshot their travel: correcting one center must not change the evidence
 * for the next one. Selected physical positions use a stronger attenuation. */
static void reject_spill(unsigned layout, unsigned row, unsigned keys, const int32_t field[], int32_t flux[],
                         const uint16_t actuation[]) {
    static const uint16_t FW_SCAN_TABLE(sensitive)[2][6] = {
        {0, 0x1000, 0x1002, 0x0802, 0x1402, 0x0028},
        {0, 0x1000, 0x1002, 0x1002, 0x2802, 0x0028},
    };
    int32_t before[16];
    memcpy(before, flux, keys * sizeof(*flux));
    for (unsigned i = 1; i + 1 < keys; ++i) {
        int32_t f = before[i];
        if (f <= 0 || before[i - 1] <= (actuation ? actuation[i - 1] : 650) ||
            before[i + 1] <= (actuation ? actuation[i + 1] : 650))
            continue;
        int32_t inside = (int32_t)(magnitude(field[i]) + magnitude(field[i + 1]));
        int32_t outside = (int32_t)(magnitude(field[i - 1]) + magnitude(field[i + 2]));
        if (outside <= inside)
            continue;
        int strong = sensitive[layout == 2][row] & (1U << i);
        int knee = strong ? 1200 : 600, ceiling = strong ? 1300 : 1000;
        if (f >= ceiling)
            continue;
        if (f <= knee)
            f = f * f / (2 * knee);
        else
            f = f * (ceiling - knee + f - knee) / (2 * (ceiling - knee));
        if (strong && f < 500)
            f = 2 * f - 500;
        flux[i] = f;
    }
    for (unsigned i = 0; i < keys; ++i)
        if (flux[i] < 0)
            flux[i] = 0;
}

/* While a row still carries field, preserve a held key with nonzero evidence
 * through a short dip below release. At true zero, do not prop up the key. */
static void stabilize_held(fw_classic_neighbors_t *state, unsigned keys, const int32_t field[], int32_t flux[],
                           const uint8_t down[], const uint16_t actuation[], const uint16_t hysteresis[]) {
    unsigned k = 0;
    while (k < keys && !down[k])
        ++k;
    if (k == keys)
        return;
    uint32_t total = 0;
    for (unsigned i = 0; i <= keys; ++i)
        total += magnitude(field[i]);
    if ((int32_t)total < 150)
        return;
    for (; k < keys; ++k) {
        if (!down[k])
            continue;
        if (flux[k] == 0) {
            state->below_release[k] = 0;
            continue;
        }
        int act = actuation ? actuation[k] : 650;
        int depth = hysteresis ? hysteresis[k] : 216;
        int release = act - depth;
        if (flux[k] < release) {
            if (state->below_release[k] < UINT8_MAX)
                ++state->below_release[k];
            if (state->below_release[k] > 15)
                continue;
        } else {
            state->below_release[k] = 0;
        }
        int floor = release + (depth < 32 ? 8 : depth / 4);
        if (floor > act - 16)
            floor = act - 16;
        if (flux[k] < floor)
            flux[k] = floor;
    }
}

void FW_SCAN_CODE(fw_classic_neighbors)(fw_classic_neighbors_t *state, unsigned layout, unsigned row, unsigned keys,
                                        const int32_t field[], int32_t flux[], const uint8_t down[],
                                        const uint16_t actuation[], const uint16_t hysteresis[]) {
    if (keys == 0 || keys > 16 || row >= 6 || layout < 1 || layout > 3)
        return;
    reject_plateaus(keys, field, flux, down);
    reject_spill(layout, row, keys, field, flux, actuation);
    stabilize_held(state, keys, field, flux, down, actuation, hysteresis);
}
