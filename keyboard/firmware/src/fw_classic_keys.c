#include "fw_classic_keys.h"
#include "fw_scan_hot.h"

/* Row gain followed by the extra gain for keys whose wide cap places the switch off centre. */
void fw_classic_key_weight(fw_classic_settings_t *s, unsigned layout, unsigned row, unsigned col) {
    if (layout < 1 || layout > 3 || row >= 6 || col >= 16)
        return;
    static const uint16_t FW_SCAN_TABLE(row_gain)[] = {256, 253, 250, 246, 242, 234};
    static const uint16_t FW_SCAN_TABLE(wide)[2][6] = {
        {0, 1U << 13, 1U | (1U << 13), 1U | (1U << 12), 1U | (1U << 11), 1U},
        {0, 1U << 13, 1U | (1U << 13), 1U | (1U << 13), 1U | (1U << 12), 1U},
    };
    unsigned gain = row_gain[row];
    if (wide[layout == 2U][row] & (1U << col))
        gain = (gain * 224U) >> 8;
    s->actuation[col] = (s->actuation[col] * gain) >> 8;
    s->hysteresis[col] = (s->hysteresis[col] * gain) >> 8;
}

void fw_classic_key_load(fw_classic_settings_t *s, fw_classic_keys_t *k, unsigned layout, unsigned row, unsigned col,
                         bool has_table, uint32_t word) {
    if (row >= 6 || col >= 16)
        return;
    unsigned type = has_table ? word >> 30 : 0;
    bool specified = type == 1 || type == 2;
    s->actuation[col] = specified ? word & 0xfffU : 650;
    s->hysteresis[col] = specified ? (word >> 12) & 0xfffU : 216;
    s->rapid[col] = type == 2;
    k->reversal[col] = (fw_classic_reversal_t){.armed = 1};
    if (type == 2) {
        /* Reversals use unweighted travel. Changing to rapid does not cancel an existing
         * fixed proposal; it still contributes to row occupancy until a rapid edge commits. */
        s->sensitivity[col] = s->hysteresis[col];
    } else {
        k->pending[col] = k->down[col];
        k->scans[col] = 0;
    }
    if (has_table)
        fw_classic_key_weight(s, layout, row, col);
}

static int press_threshold(int a, unsigned row, unsigned occupied) {
    int extra, cap;
    if (row == 5 && occupied > 1) {
        extra = 400;
        cap = 1000;
    } else if (row != 5 && occupied > 7) {
        extra = 1000;
        cap = 1200;
    } else if (row != 5 && occupied > 3) {
        extra = 400;
        cap = 800;
    } else {
        return a;
    }
    return a + extra < cap ? a + extra : cap;
}

/* An armed first press is immediate. A partial release/re-press needs five consecutive
 * proposals. At very high travel the switch remains down regardless of a small reversal. */
static bool rapid_step(fw_classic_reversal_t *r, bool down, int32_t f, int press, int a, int h, int sensitivity) {
    int reset = a - h > 120 ? a - h : 120;
    bool proposed = down, immediate = false;
    if (f > 820) {
        proposed = true;
        if (!down && r->armed) {
            r->armed = 0;
            r->trough = f;
            immediate = true;
        }
        if (r->peak < f)
            r->peak = f;
    } else if (down) {
        if (r->peak < f)
            r->peak = f;
        if (f < (int32_t)((uint32_t)r->peak - (uint32_t)sensitivity) || (f <= a && f < reset))
            proposed = false;
    } else if (r->armed && f > press) {
        r->armed = 0;
        r->trough = f;
        proposed = immediate = true;
    } else {
        if (!r->armed) {
            if (r->trough > f)
                r->trough = f;
            proposed = f > (int32_t)((uint32_t)r->trough + (uint32_t)sensitivity);
        }
        if (f <= press && f < reset) {
            proposed = false;
            r->armed = 1;
            r->trough = r->peak = 0;
        }
    }

    if (proposed == down) {
        r->proposed = down;
        r->scans = 0;
    } else if (immediate) {
        r->peak = f;
        return true;
    } else if (r->proposed != proposed) {
        r->proposed = proposed;
        r->scans = 1;
    } else if (r->scans < 4) {
        r->scans++;
    } else {
        if (proposed)
            r->peak = f;
        else
            r->trough = f;
        return proposed;
    }
    return down;
}

void FW_SCAN_CODE(fw_classic_keys_step)(fw_classic_keys_t *k, const fw_classic_settings_t *s, unsigned row, unsigned n,
                                        unsigned held_before, const int32_t flux[]) {
    if (row >= 6 || n > 16)
        return;
    unsigned occupied = 0;
    for (unsigned i = 0; i < n; ++i)
        occupied += k->down[i] || k->pending[i];
    for (unsigned i = 0; i < n; ++i) {
        int a = s->actuation[i], h = s->hysteresis[i];
        int press = press_threshold(a, row, occupied);
        if (s->rapid[i]) {
            bool next = rapid_step(&k->reversal[i], k->down[i], flux[i], press, a, h, s->sensitivity[i]);
            if (next != k->down[i]) {
                k->down[i] = k->pending[i] = next;
                k->scans[i] = 0;
            }
        } else {
            bool next = flux[i] > (k->down[i] ? a - h : press);
            unsigned duration = held_before > 3 ? 16U : 2U;
            if (next == k->down[i]) {
                k->pending[i] = next;
                k->scans[i] = 0;
            } else if (next != k->pending[i]) {
                k->pending[i] = next;
                k->scans[i] = 1;
            } else if (k->scans[i] < duration) {
                k->scans[i]++;
            } else {
                k->down[i] = next;
            }
        }
    }
}
