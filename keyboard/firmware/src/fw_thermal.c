#include "fw_thermal.h"
#include "fw_scanner.h" /* layout row lengths and physical sensor slots */

#include <stdlib.h>
#include <string.h>

/* Signed upper median. Selection avoids sorting the entire 112-channel window. */
static int32_t median(int32_t *a, unsigned n) {
    if (!n)
        return 0;
    int lo = 0, hi = (int)n - 1, mid = (int)n / 2;
    while (lo < hi) {
        int32_t pivot = a[(lo + hi) / 2];
        int i = lo, j = hi;
        while (i <= j) {
            while (a[i] < pivot)
                i++;
            while (a[j] > pivot)
                j--;
            if (i <= j) {
                int32_t t = a[i];
                a[i++] = a[j];
                a[j--] = t;
            }
        }
        if (mid <= j)
            hi = j;
        else if (mid >= i)
            lo = i;
        else
            break;
    }
    return a[mid];
}

/* Relative gain is poorly conditioned near ADC midrail. Ignore those sensors rather
 * than amplifying a few counts of noise into a large apparent gain change. */
static void reset_window(fw_thermal_state_t *s, const uint16_t *baseline) {
    for (unsigned i = 0; i < 112; i++) {
        int scale = (int)baseline[i] - 2048;
        s->inverse_center[i] = abs(scale) < 200 ? 0 : 1048576 / scale;
    }
    memset(s->sum, 0, sizeof s->sum);
    s->initialized = s->windows = 0;
    s->gain_q10 = 0;
    s->sustained_drift = false;
}

void fw_thermal_acquired(fw_thermal_state_t *s, const uint16_t baseline[160]) {
    s->latched_drift = s->deferred_held = s->require_strong = false;
    memset(s->protect_held, 0, sizeof s->protect_held);
    reset_window(s, baseline);
}

static void select_reference_channels(fw_thermal_state_t *s, unsigned layout) {
    static const uint8_t candidates[] = {7, 8, 23, 39, 38, 55, 56, 71, 72, 89, 103, 104};
    bool mapped[112] = {0};
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c < FW_ROW_LEN[layout][r]; c++)
            mapped[FW_SENSOR_MAP[layout][r][c]] = true;
    s->reference_count = 0;
    for (unsigned i = 0; i < sizeof candidates; i++)
        if (!mapped[candidates[i]])
            s->reference_channel[s->reference_count++] = candidates[i];
    s->layout = (uint8_t)layout;
    memset(s->sum, 0, sizeof s->sum);
    s->initialized = s->windows = 0;
}

typedef struct {
    const uint16_t *raw, *baseline;
    const fw_thermal_key_t (*key)[16];
    uint8_t layout;
    unsigned held;
} frame_t;

static bool quiet(const frame_t *f, unsigned r, unsigned c) {
    return (!c || f->key[r][c - 1].flux <= 100) && (c + 1 >= FW_ROW_LEN[f->layout][r] || f->key[r][c].flux <= 100);
}

static int32_t estimate_gain(fw_thermal_state_t *s, const frame_t *f) {
    int32_t *values = s->select;
    unsigned n = 0;
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c < FW_ROW_LEN[f->layout][r]; c++) {
            unsigned i = FW_SENSOR_MAP[f->layout][r][c];
            if (quiet(f, r, c) && s->inverse_center[i])
                values[n++] = (s->inverse_center[i] * ((int)f->raw[i] - f->baseline[i])) >> 10;
        }
    return median(values, n);
}

/* Test whether the changes project onto the signed resting magnetic field, with
 * support in the bottom rows rather than a disturbance confined to the top. */
static bool field_correlation(const fw_thermal_state_t *s, const frame_t *f, const int16_t *reference) {
    int64_t dot = 0, field_energy = 0, change_energy = 0;
    int32_t top = 0, bottom = 0;
    unsigned top_n = 0, bottom_n = 0;
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c < FW_ROW_LEN[f->layout][r]; c++) {
            unsigned i = FW_SENSOR_MAP[f->layout][r][c];
            if (!quiet(f, r, c) || !s->inverse_center[i])
                continue;
            int field = (int)f->baseline[i] - 2048;
            int change = (int)f->raw[i] - (reference ? (int)reference[i] : (int)f->baseline[i]);
            dot += (int64_t)field * change;
            field_energy += (int64_t)field * field;
            change_energy += (int64_t)change * change;
            int gain = abs((s->inverse_center[i] * change) >> 10);
            if (r < 2) {
                top += gain;
                top_n++;
            } else if (r >= 4) {
                bottom += gain;
                bottom_n++;
            }
        }
    /* These products fit int64 even at 12-bit ADC extremes for 112 channels. */
    return change_energy && top_n && bottom_n && dot * dot >= ((change_energy * field_energy) >> 10) * 205 &&
           top * bottom_n <= 2 * bottom * top_n;
}

static bool row_drift(fw_thermal_state_t *s, const frame_t *f, bool crowded, bool *settled) {
    unsigned rows = 0, fast = 0, either = 0;
    const int16_t *near = s->frames[0][s->next[0]], *far = s->frames[1][s->next[1]];
    for (unsigned r = 0; r < 6; r++) {
        int32_t *a = s->select, *b = s->select + FW_MATRIX_MAX_COLS;
        unsigned n = 0;
        for (unsigned c = 0; c < FW_ROW_LEN[f->layout][r]; c++) {
            unsigned i = FW_SENSOR_MAP[f->layout][r][c];
            if (!quiet(f, r, c) || !s->inverse_center[i])
                continue;
            a[n] = (s->inverse_center[i] * ((int)f->raw[i] - near[i])) >> 10;
            b[n] = (s->inverse_center[i] * ((int)f->raw[i] - far[i])) >> 10;
            n++;
        }
        if (n >= 3) {
            bool recent = abs(median(a, n)) > 10;
            either += recent || abs(median(b, n)) > 16;
            fast += recent;
            rows++;
        }
    }
    *settled = !either;
    if (rows < 3)
        return false;
    if (fast * 3 >= rows * 2)
        return field_correlation(s, f, near);
    return !crowded && either * 3 >= rows * 2 && field_correlation(s, f, far);
}

static int reference_change(fw_thermal_state_t *s, const frame_t *f, const uint16_t *history) {
    int32_t *deltas = s->select;
    for (unsigned j = 0; j < s->reference_count; j++) {
        int previous = history ? history[j] : f->baseline[s->reference_channel[j]] * 16;
        deltas[j] = abs(s->sum[j] - previous);
    }
    return median(deltas, s->reference_count) / 16;
}

static void snapshot(fw_thermal_state_t *s, const frame_t *f, unsigned age, unsigned slot) {
    memcpy(s->history[age][slot], s->sum, sizeof s->sum);
    for (unsigned i = 0; i < 112; i++)
        s->frames[age][slot][i] = (int16_t)f->raw[i];
}

/* Every 16 sweeps compare the background field against rolling 100 ms and 500 ms
 * windows. With an accessory over the reference sensors use changes shared by the
 * quiet keyboard rows instead. A chord needs substantially more evidence. */
static void detect(fw_thermal_state_t *s, const frame_t *f, uint64_t ms, bool module) {
    for (unsigned j = 0; j < s->reference_count; j++)
        s->sum[j] += f->raw[s->reference_channel[j]];
    if (++s->windows < 16)
        return;
    s->windows = 0;
    s->gain_q10 = estimate_gain(s, f);
    if (!s->initialized) {
        for (unsigned g = 0; g < 2; g++) {
            for (unsigned k = 0; k < 4; k++)
                snapshot(s, f, g, k);
            s->history_ms[g] = ms;
        }
        s->initialized = 1;
    }
    int near = reference_change(s, f, s->history[0][s->next[0]]),
        far = reference_change(s, f, s->history[1][s->next[1]]);
    s->sustained_drift = s->reference_count >= 6 && reference_change(s, f, NULL) > 8;
    bool drift = s->reference_count >= 6 && (near > 6 || far > 8), settled = far < 4;
    if (f->held > 2)
        s->chord_until = ms + 600;
    if (s->module_mode != module)
        s->evidence = 0;
    s->module_mode = module;
    bool chord = s->chord_until > ms;
    if (module) {
        drift = row_drift(s, f, chord, &settled);
        s->sustained_drift = !chord && abs(s->gain_q10) > 25 && field_correlation(s, f, NULL);
    }
    if (!drift)
        s->evidence = 0;
    else if (s->evidence < 255)
        s->evidence++;
    for (unsigned g = 0; g < 2; g++)
        if (ms - s->history_ms[g] >= (g ? 125U : 25U)) {
            snapshot(s, f, g, s->next[g]);
            s->next[g] = (s->next[g] + 1) & 3;
            s->history_ms[g] = ms;
        }
    memset(s->sum, 0, sizeof s->sum);
    if (s->evidence >= (module ? (chord ? 10 : 3) : 2)) {
        s->disturbance_until = ms + 150;
        s->latched_drift = true;
    }
    s->moving = s->disturbance_until > ms;
    if (s->moving || !settled)
        s->last_drift = ms;
    s->require_strong = s->moving || s->latched_drift || s->deferred_held ||
                        (abs(s->gain_q10) > 25 && s->sustained_drift) || (s->refreshed && ms - s->last_refresh < 1000);
}

static bool busy(const fw_thermal_key_t *k) {
    return k->down || k->pending || k->masked;
}

bool fw_thermal_after_scan(fw_thermal_state_t *s, uint8_t layout, uint32_t now_ms, const uint16_t raw[160],
                           uint16_t baseline[160], const fw_thermal_key_t keys[6][16], bool module_fields) {
    /* Extend the wrapping hardware clock so an expired guard cannot spring back to life
     * at 24 or 49 days. We sample it at least once per counter revolution while running. */
    s->clock_ms += (uint32_t)(now_ms - (uint32_t)s->clock_ms);
    uint64_t ms = s->clock_ms;
    if (layout < 1 || layout > 3)
        return false;
    if (s->layout != layout)
        select_reference_channels(s, layout);
    frame_t frame = {.layout = layout, .raw = raw, .baseline = baseline, .key = keys};
    bool occupied = false;
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c + 1 < FW_ROW_LEN[layout][r]; c++) {
            frame.held += !!keys[r][c].down;
            occupied |= busy(&keys[r][c]);
        }
    detect(s, &frame, ms, module_fields);
    /* During drift, follow quiet sensors by a single count per sweep. Never follow a
     * sensor bordering a held, masked, or still-moving key. */
    bool stable = true;
    for (unsigned r = 0; r < 6; r++) {
        unsigned n = FW_ROW_LEN[layout][r];
        for (unsigned c = 0; c < n; c++) {
            if (s->require_strong) {
                bool at_rest = true;
                for (unsigned k = c ? c - 1 : 0; k <= c && k + 1 < n; k++)
                    if (busy(&keys[r][k]) || abs((int)keys[r][k].flux - s->previous_flux[r][k]) > 4)
                        at_rest = false;
                if (at_rest) {
                    unsigned i = FW_SENSOR_MAP[layout][r][c];
                    baseline[i] += (raw[i] > baseline[i]) - (raw[i] < baseline[i]);
                }
            }
            if (c + 1 < n) {
                const fw_thermal_key_t *k = &keys[r][c];
                if (k->down != k->pending || (s->collected_count && abs((int)k->flux - s->window_flux[r][c]) > 30))
                    stable = false;
            }
        }
    }
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c < 16; c++)
            s->previous_flux[r][c] = keys[r][c].flux;
    if (frame.held > 2)
        s->last_chord = ms;
    if (s->moving || ms - s->last_drift < 300 || !stable || ms - s->last_chord < 600 ||
        !(s->latched_drift || (abs(s->gain_q10) > 25 && s->sustained_drift) || (s->deferred_held && !occupied))) {
        s->collected_count = 0;
        return false;
    }
    if (!s->collected_count) {
        memcpy(s->window_flux, s->previous_flux, sizeof s->window_flux);
        memset(s->collected, 0, sizeof s->collected);
    }
    for (unsigned i = 0; i < 112; i++)
        s->collected[i] += raw[i];
    if (++s->collected_count < 16)
        return false;
    /* A complete stable window can rebaseline the board. A held sensor cannot be
     * measured at rest, so apply only the proportional change estimated elsewhere. */
    bool blocked[112] = {0};
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c + 1 < FW_ROW_LEN[layout][r]; c++)
            if (busy(&keys[r][c])) {
                blocked[FW_SENSOR_MAP[layout][r][c]] = blocked[FW_SENSOR_MAP[layout][r][c + 1]] = true;
            }
    for (unsigned i = 0; i < 112; i++) {
        int next =
            blocked[i] ? baseline[i] + s->gain_q10 * ((int)baseline[i] - 2048) / 1024 : (s->collected[i] + 8) / 16;
        baseline[i] = next < 0 ? 0 : next > 4095 ? 4095 : (uint16_t)next;
    }
    s->collected_count = 0;
    s->refresh_count++;
    s->refreshed = true;
    s->last_refresh = ms;
    s->latched_drift = false;
    s->require_strong = s->deferred_held = occupied;
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c < 16; c++)
            s->protect_held[r][c] = keys[r][c].down;
    reset_window(s, baseline);
    return true;
}
