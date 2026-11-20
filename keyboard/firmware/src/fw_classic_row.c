#include "fw_classic_row.h"
#include "fw_scan_hot.h"

/* Shrink toward zero without changing the polarity of the reading. */
static int32_t deadband(int32_t value, int32_t band) {
    return value > band ? value - band : value < -band ? value + band : 0;
}

/* Remove residual common field preferentially from the active sensors. The
 * first correction is proportional to field strength, limited to 180 counts
 * per sensor. Divide what's left equally amongst the sensors not at that limit. */
static void balance(unsigned width, int32_t field[]) {
    int32_t total = 0, weight = 0;
    for (unsigned i = 0; i < width; i++) {
        int32_t magnitude = field[i] < 0 ? -field[i] : field[i];
        total += field[i];
        if (magnitude > 20)
            weight += magnitude;
    }
    if (!total || !weight)
        return;

    int32_t remainder = total, eligible = 0;
    uint32_t eligible_mask = 0;
    for (unsigned i = 0; i < width; i++) {
        int32_t magnitude = field[i] < 0 ? -field[i] : field[i];
        if (magnitude <= 20)
            continue;
        /* Multiply in uint32_t so a product beyond int32_t wraps modulo 2^32 (defined behaviour) instead of
         * overflowing; host tests and the RP2040 then divide the same signed value. */
        int32_t product = (int32_t)((uint32_t)total * (uint32_t)magnitude);
        int32_t share = product / weight;
        if (share >= 180) {
            share = 180;
        } else if (share <= -180) {
            share = -180;
        } else {
            eligible_mask |= UINT32_C(1) << i;
            eligible++;
        }
        remainder -= share;
        field[i] -= share;
    }
    if (eligible && (remainder >= eligible || remainder <= -eligible)) {
        int32_t share = remainder / eligible;
        for (unsigned i = 0; i < width; i++)
            if (eligible_mask & (UINT32_C(1) << i))
                field[i] -= share;
    }
}

void FW_SCAN_CODE(fw_classic_row)(unsigned row, unsigned width, const int32_t *sensors, int32_t *conditioned,
                                  int32_t *flux) {
    if (row >= 6 || width < 2 || width > 17)
        return;

    int32_t sum = 0, tilt = 0;
    unsigned silent = 0;
    for (unsigned i = 0; i < width; i++) {
        sum += sensors[i];
        silent += sensors[i] == 0;
        /* Omit the middle sensor of odd-width rows. */
        if (i < width / 2)
            tilt += sensors[i];
        else if (i >= (width + 1) / 2)
            tilt -= sensors[i];
    }
    int32_t offset = (silent <= 3 && (sum > 100 || sum < -100)) ? sum / (int32_t)width : 0;
    int32_t band = tilt > 100 ? 100 - 20 * (int32_t)row : 0;
    for (unsigned i = 0; i < width; i++)
        conditioned[i] = deadband(sensors[i] - offset, band);
    balance(width, conditioned);

    unsigned first = width, last = 0;
    sum = 0;
    for (unsigned i = 0; i < width; i++) {
        conditioned[i] = deadband(conditioned[i], 50);
        sum += conditioned[i];
        if (conditioned[i]) {
            if (i < first)
                first = i;
            last = i;
        }
    }
    /* A key sees the field to its left minus the field to its right. Outside
     * the occupied span, that difference comes only from row imbalance. */
    int32_t left = 0;
    for (unsigned i = 0; i + 1 < width; i++) {
        left += conditioned[i];
        flux[i] = i >= first && i < last ? 2 * left - sum : 0;
    }
}
