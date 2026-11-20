#include "fw_rest_runtime.h"
#include "fw_rest_internal.h"
#include "fw_scanner.h"

#include <stdlib.h>
#include <string.h>

#define SUSPECT_LIFETIME_MS 300000U

void fw_rest_begin(fw_rest_session_t *s, uint8_t layout, const fw_rest_guard_t *guard, bool bridge_connected,
                   bool qc_mode, uint16_t thermal_generation, uint32_t now_ms) {
    memset(s, 0, sizeof *s);
    if (layout < 1 || layout > 3)
        return;
    s->layout = layout;
    s->start_ms = s->last_pressed_ms = now_ms;
    s->expire_from_ms = now_ms;
    s->thermal_generation = thermal_generation;
    s->thermal_shift = guard->thermal_shift;
    s->no_match = guard->match != FW_REST_MATCH;
    memcpy(s->key, guard->key, sizeof s->key);
    for (unsigned r = 0; r < 6; r++) {
        for (unsigned k = 0; k + 1U < FW_ROW_LEN[layout][r]; k++) {
            bool held = s->key[r][k] == FW_REST_HELD;
            s->inhibit[r][k] = held;
            if (held && bridge_connected && !qc_mode)
                s->inhibit_all_new = true;
            s->tracked_keys += s->key[r][k] != FW_REST_CLEAR;
        }
    }
}

static void observe(fw_rest_sensor_watch_t *w, uint16_t raw, int32_t delta, uint32_t now) {
    if (!w->samples || abs(delta - w->initial_delta) > 30 || w->samples == UINT16_MAX) {
        w->samples = 0;
        w->initial_delta = delta;
        w->since_ms = now;
        w->raw_sum = 0;
    }
    w->raw_sum += raw;
    w->samples++;
}

static void adopt(fw_rest_session_t *s, unsigned r, unsigned c, uint32_t now, uint16_t *baseline) {
    unsigned sensor = FW_SENSOR_MAP[s->layout][r][c];
    fw_rest_sensor_watch_t *w = &s->watch[sensor];
    if (w->samples)
        baseline[sensor] = (uint16_t)((w->raw_sum + w->samples / 2U) / w->samples);
    w->samples = 0;
    if (w->reacquire) {
        w->reacquire = false;
        s->reacquiring--;
    }
    s->suppress_window = true;
    s->last_adoption_ms = now;
    for (unsigned k = c ? c - 1 : 0; k <= c && k + 1 < FW_ROW_LEN[s->layout][r]; k++) {
        s->suppress_mask[r] |= (uint16_t)(1U << k);
        s->suppress_key_from_ms[r][k] = now;
    }
}

typedef struct {
    int32_t positive, negative;
    bool occupied, suspect, suppress, recovering, stable;
} field_t;

static bool clear_key(const fw_rest_session_t *s, unsigned r, unsigned c, const uint8_t *down, const uint8_t *pending,
                      const int32_t *travel) {
    return s->key[r][c] != FW_REST_HELD && !down[c] && !pending[c] && travel[c] < 120;
}

static bool recover_row(fw_rest_session_t *s, unsigned row, uint32_t now, const uint16_t *raw, const int32_t *delta,
                        const uint8_t *down, const uint8_t *pending, bool thermal_busy, bool learning,
                        uint16_t *baseline) {
    const uint8_t *map = FW_SENSOR_MAP[s->layout][row];
    unsigned n = FW_ROW_LEN[s->layout][row];
    uint8_t *key = s->key[row];
    int32_t values[FW_MATRIX_MAX_COLS], travel[FW_MATRIX_MAX_COLS];
    uint8_t labels[FW_MATRIX_MAX_COLS];
    field_t fields[FW_MATRIX_MAX_COLS + 1] = {0};
    for (unsigned c = 0; c < n; c++)
        values[c] = delta[map[c]];
    unsigned groups = fw_rest_fields(values, n, travel, labels);
    for (unsigned c = 0; c + 1 < n; c++) {
        if (!labels[c] || labels[c] != labels[c + 1])
            continue;
        field_t *f = &fields[labels[c]];
        if (travel[c] > 0)
            f->positive += travel[c];
        else
            f->negative -= travel[c];
        f->occupied |= down[c] || pending[c] ||
                       ((s->inhibit_all_new || key[c] == FW_REST_HELD || s->thermal_shift) && travel[c] > 120);
        f->suspect |= key[c] == FW_REST_UNCERTAIN;
    }
    for (unsigned i = 1; i <= groups; i++) {
        field_t *f = &fields[i];
        f->suppress = !thermal_busy && (learning || f->suspect) && !f->occupied &&
                      f->negative > (s->thermal_shift ? 200 : 40) && f->negative > 2 * f->positive;
        f->recovering = f->suppress && f->negative > (s->thermal_shift ? 200 : 120);
        f->stable = f->recovering;
    }
    bool watch[FW_MATRIX_MAX_COLS];
    for (unsigned c = 0; c < n; c++)
        watch[c] = fields[labels[c]].recovering || s->watch[map[c]].reacquire;
    bool held_remains = false;
    for (unsigned c = 0; c + 1 < n; c++) {
        s->inhibit[row][c] = false;
        if (key[c] != FW_REST_HELD)
            continue;
        if (now - s->start_ms >= 20000U) {
            baseline[map[c]] = raw[map[c]];
            baseline[map[c + 1]] = raw[map[c + 1]];
            key[c] = FW_REST_UNCERTAIN;
            s->expire_from_ms = now;
            continue;
        }
        bool quiet = abs(travel[c]) < 120;
        if (quiet && !s->quiet[row][c])
            s->quiet_from_ms[row][c] = now;
        s->quiet[row][c] = quiet;
        if (quiet && !thermal_busy && now - s->quiet_from_ms[row][c] >= 100U) {
            key[c] = FW_REST_CLEAR;
            s->tracked_keys--;
            for (unsigned j = c; j <= c + 1; j++) {
                if (!s->watch[map[j]].reacquire) {
                    s->watch[map[j]].reacquire = true;
                    s->reacquiring++;
                }
            }
        } else {
            s->inhibit[row][c] = true;
            held_remains = true;
        }
    }
    for (unsigned c = 0; c < n; c++) {
        fw_rest_sensor_watch_t *w = &s->watch[map[c]];
        if (!watch[c])
            w->samples = 0;
        else
            observe(w, raw[map[c]], values[c], now);
        if (!w->samples || now - w->since_ms < 100U)
            fields[labels[c]].stable = false;
    }
    for (unsigned i = 1; i <= groups; i++) {
        if (!fields[i].stable)
            continue;
        for (unsigned c = 0; c + 1 < n; c++)
            if (labels[c] == i && labels[c + 1] == i && key[c] == FW_REST_UNCERTAIN) {
                key[c] = FW_REST_CLEAR;
                s->tracked_keys--;
            }
        for (unsigned c = 0; c < n; c++)
            if (labels[c] == i)
                adopt(s, row, c, now, baseline);
        fields[i].suppress = false;
    }
    for (unsigned c = 0; c + 1 < n; c++) {
        bool left = fields[labels[c]].suppress, right = fields[labels[c + 1]].suppress;
        if ((s->thermal_shift ? left && right && labels[c] == labels[c + 1] : left || right) ||
            (s->suppress_mask[row] & (1U << c)))
            s->inhibit[row][c] = true;
    }
    for (unsigned c = 0; c < n; c++) {
        fw_rest_sensor_watch_t *w = &s->watch[map[c]];
        if (w->reacquire && w->samples && now - w->since_ms >= 100U && abs(values[c]) < 120 &&
            (!c || clear_key(s, row, c - 1, down, pending, travel)) &&
            (c + 1 == n || clear_key(s, row, c, down, pending, travel)))
            adopt(s, row, c, now, baseline);
    }
    return held_remains;
}

void fw_rest_recover(fw_rest_session_t *s, uint32_t now_ms, const uint16_t raw[FW_NUM_SENSORS],
                     const int32_t delta[FW_NUM_SENSORS], const uint8_t confirmed[6][16], const uint8_t proposed[6][16],
                     bool thermal_busy, uint16_t thermal_generation, uint16_t baseline[FW_NUM_SENSORS]) {
    if (!s->layout)
        return;
    if (thermal_generation != s->thermal_generation) {
        s->thermal_generation = thermal_generation;
        s->expire_from_ms = now_ms;
    }
    bool learning = s->no_match && now_ms - s->start_ms < 30000U;
    /* Never compare an unarmed deadline: a long-lived, idle keyboard will cross both the signed
     * half-range and rollover. Retire expired windows before another adoption can open one. */
    if (s->suppress_window) {
        if (now_ms - s->last_adoption_ms >= 50U) {
            s->suppress_window = false;
            memset(s->suppress_mask, 0, sizeof s->suppress_mask);
        } else {
            for (unsigned r = 0; r < 6; r++) {
                if (!s->suppress_mask[r])
                    continue;
                for (unsigned k = 0; k < 16; k++)
                    if ((s->suppress_mask[r] & (1U << k)) && now_ms - s->suppress_key_from_ms[r][k] >= 50U)
                        s->suppress_mask[r] &= (uint16_t) ~(1U << k);
            }
        }
    }
    if (now_ms - s->expire_from_ms >= SUSPECT_LIFETIME_MS && (s->tracked_keys || s->reacquiring)) {
        for (unsigned r = 0; r < 6; r++)
            for (unsigned c = 0; c + 1U < FW_ROW_LEN[s->layout][r]; c++)
                if (s->key[r][c] == FW_REST_UNCERTAIN) {
                    s->key[r][c] = FW_REST_CLEAR;
                    s->tracked_keys--;
                }
        for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
            s->watch[i].reacquire = false;
        s->reacquiring = 0;
    }
    if (!s->tracked_keys && !s->reacquiring && !learning && !s->suppress_window) {
        if (!s->finished) {
            s->finished = true;
            s->no_match = false;
            s->inhibit_all_new = false;
            memset(s->inhibit, 0, sizeof s->inhibit);
        }
        return;
    }
    bool held = false;
    for (unsigned r = 0; r < 6; r++)
        held |= recover_row(s, r, now_ms, raw, delta, confirmed[r], proposed[r], thermal_busy, learning, baseline);
    if (!held || now_ms - s->start_ms >= 5000U)
        s->inhibit_all_new = false;
}

bool fw_rest_idle(fw_rest_session_t *s, uint32_t now_ms, bool any_pressed, const uint16_t raw[FW_NUM_SENSORS],
                  uint16_t baseline[FW_NUM_SENSORS]) {
    if (!s->layout)
        return false;
    if (any_pressed) {
        s->last_pressed_ms = now_ms;
        return false;
    }
    if (now_ms - s->last_pressed_ms < 480000U || now_ms - s->last_idle_sample_ms < 120000U)
        return false;
    s->last_idle_sample_ms = now_ms;
    memcpy(s->idle[s->idle_next], raw, sizeof s->idle[0]);
    s->idle_next = (s->idle_next + 1U) % 9U;
    if (s->idle_count < 9)
        s->idle_count++;
    if (s->idle_count < 9)
        return false;
    for (unsigned i = 0; i < FW_NUM_SENSORS; i++) {
        uint16_t sorted[9];
        for (unsigned j = 0; j < 9; j++) {
            unsigned k = j;
            while (k && s->idle[j][i] < sorted[k - 1]) {
                sorted[k] = sorted[k - 1];
                k--;
            }
            sorted[k] = s->idle[j][i];
        }
        baseline[i] = sorted[4];
    }
    bool demoted = false;
    for (unsigned r = 0; r < 6; r++)
        for (unsigned c = 0; c + 1U < FW_ROW_LEN[s->layout][r]; c++) {
            if (s->key[r][c] == FW_REST_HELD) {
                s->key[r][c] = FW_REST_UNCERTAIN;
                s->inhibit[r][c] = false;
                demoted = true;
            }
        }
    memset(s->watch, 0, sizeof s->watch);
    s->reacquiring = 0;
    s->inhibit_all_new = false;
    s->no_match = false;
    if (demoted) {
        s->expire_from_ms = now_ms;
        return false;
    }
    if (s->rest_saved)
        return false;
    s->rest_saved = true;
    return true;
}
