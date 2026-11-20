#include "fw_rest.h"
#include "fw_rest_internal.h"
#include "fw_scanner.h"

#include <stdlib.h>
#include <string.h>

#define REST_MAGIC 0x52455354U

static void put16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}
static void put32(uint8_t *p, uint32_t v) {
    put16(p, (uint16_t)v);
    put16(p + 2, (uint16_t)(v >> 16));
}
static uint16_t get16(const uint8_t *p) {
    return (uint16_t)((uint16_t)p[0] | (uint16_t)p[1] << 8);
}
static uint32_t get32(const uint8_t *p) {
    return (uint32_t)get16(p) | (uint32_t)get16(p + 2) << 16;
}

static uint32_t record_checksum(const uint8_t *p) {
    uint32_t h = get32(p) + get16(p + 4) + p[6] + p[7];
    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        h = h * 31U + get16(p + 8 + 2 * i);
    return h;
}

int fw_rest_encode(uint8_t layout, uint8_t feel, const int16_t reference[FW_REST_CHANNELS],
                   uint8_t out[FW_REST_BYTES]) {
    if (!out || !reference)
        return -1;
    put32(out, REST_MAGIC);
    put16(out + 4, 1);
    out[6] = layout;
    out[7] = feel;
    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        put16(out + 8 + 2 * i, (uint16_t)reference[i]);
    put32(out + 232, record_checksum(out));
    return 0;
}

int fw_rest_decode(const uint8_t record[FW_REST_BYTES], uint8_t layout, uint8_t feel, int16_t out[FW_REST_CHANNELS]) {
    if (!record || !out || get32(record) != REST_MAGIC || get16(record + 4) != 1 || record[6] != layout ||
        record[7] != feel)
        return -1;
    uint32_t stored = get32(record + 232);
    if (record_checksum(record) != stored) {
        uint32_t byte_checksum = 0;
        for (unsigned i = 0; i < 232; i++)
            byte_checksum = byte_checksum * 31U + record[i];
        if (stored != byte_checksum)
            return -1;
    }
    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        out[i] = (int16_t)get16(record + 8 + 2 * i);
    return 0;
}

static int32_t upper_median(int32_t *v, unsigned n) {
    for (unsigned i = 1; i < n; i++) {
        unsigned j = i;
        int32_t x = v[i];
        while (j && v[j - 1] > x) {
            v[j] = v[j - 1];
            --j;
        }
        v[j] = x;
    }
    return n ? v[n / 2] : 0;
}

/* Slots installed away from the keys measure board-wide thermal change independently of key travel. */
static bool background_drift(const uint16_t *mean, const int16_t *reference) {
    const uint8_t slots[] = {7, 8, 23, 38, 39, 55, 56, 71, 72, 89, 103, 104};
    int32_t change[sizeof slots];
    for (unsigned i = 0; i < sizeof slots; i++)
        change[i] = abs(mean[slots[i]] - reference[slots[i]]);
    return upper_median(change, sizeof slots) > 8;
}

static void align_reference(uint8_t layout, const uint16_t *mean, const int16_t *rest, int32_t *adjusted,
                            fw_rest_guard_t *g) {
    int32_t samples[FW_REST_CHANNELS];
    unsigned n = 0;
    for (unsigned r = 0; r < FW_MATRIX_ROWS; r++) {
        for (unsigned c = 0; c < FW_ROW_LEN[layout][r]; c++) {
            unsigned s = FW_SENSOR_MAP[layout][r][c];
            int32_t x = rest[s] - 2048;
            if (abs(x) >= 200)
                samples[n++] = (mean[s] - rest[s]) * 1024 / x;
        }
    }
    int32_t gain = upper_median(samples, n);
    n = 0;
    for (unsigned s = 0; s < FW_REST_CHANNELS; s++)
        adjusted[s] = rest[s] + (int32_t)(((int64_t)gain * (rest[s] - 2048)) / 1024);
    for (unsigned r = 0; r < FW_MATRIX_ROWS; r++) {
        for (unsigned c = 0; c < FW_ROW_LEN[layout][r]; c++) {
            unsigned s = FW_SENSOR_MAP[layout][r][c];
            samples[n++] = mean[s] - adjusted[s];
        }
    }
    g->thermal_gain_q10 = gain;
    g->thermal_offset = upper_median(samples, n);
    for (unsigned s = 0; s < FW_REST_CHANNELS; s++)
        adjusted[s] += g->thermal_offset;
}

static uint16_t adc_range(int32_t x) {
    return x < 0 ? 0 : x > 4095 ? 4095 : (uint16_t)x;
}

/* Consecutive excursions, including gaps of up to three quiet sensors, belong to one
 * local magnetic field. Compare both sides of each intervening key against its centre. */
static void classify_row(unsigned row, const uint8_t *map, unsigned n, const uint16_t *mean, const int32_t *adj,
                         unsigned threshold, bool thermal, bool replace[FW_REST_CHANNELS], fw_rest_guard_t *g) {
    int32_t delta[FW_MATRIX_MAX_COLS], travel[FW_MATRIX_MAX_COLS];
    uint8_t label[FW_MATRIX_MAX_COLS];
    bool certain[FW_MATRIX_MAX_COLS + 1] = {0};
    for (unsigned c = 0; c < n; c++)
        delta[c] = mean[map[c]] - adj[map[c]];
    fw_rest_fields(delta, n, travel, label);
    for (unsigned c = 0; c + 1 < n; c++)
        if (!thermal && travel[c] > 300)
            certain[label[c]] = true;
    for (unsigned c = 0; c < n; c++) {
        if (certain[label[c]])
            replace[map[c]] = true;
        if (travel[c] > (int32_t)threshold) {
            g->key[row][c] = certain[label[c]] ? FW_REST_HELD : FW_REST_UNCERTAIN;
            g->off_keys++;
        }
    }
}

void fw_rest_apply(uint8_t layout, uint16_t mean[FW_NUM_SENSORS], const uint8_t unstable[FW_REST_CHANNELS],
                   const int16_t reference[FW_REST_CHANNELS], fw_rest_guard_t *g) {
    memset(g, 0, sizeof *g);
    if (layout < 1 || layout > 3)
        return;
    int32_t adjusted[FW_REST_CHANNELS];
    if (reference) {
        align_reference(layout, mean, reference, adjusted, g);
        g->match = FW_REST_MATCH;
        g->thermal_shift = abs(g->thermal_gain_q10) > 40 || background_drift(mean, reference);
        unsigned count = 0;
        bool replace[FW_REST_CHANNELS] = {0};
        for (unsigned row = 0; row < FW_MATRIX_ROWS; row++) {
            unsigned n = FW_ROW_LEN[layout][row];
            classify_row(row, FW_SENSOR_MAP[layout][row], n, mean, adjusted, g->thermal_shift ? 200 : 120,
                         g->thermal_shift, replace, g);
            count += n - 1;
        }
        if (g->off_keys * 100U > count * 30U) {
            g->match = FW_REST_OTHER_FRAME;
            g->thermal_shift = false;
            memset(g->key, 0, sizeof g->key);
        } else {
            for (unsigned s = 0; s < FW_REST_CHANNELS; s++)
                if (replace[s])
                    mean[s] = adc_range(adjusted[s]);
            /* A noisy sensor is assessed against the already corrected frame. Only the recomputed
             * `adjusted` is used; the guard keeps the thermal fit from the uncorrected frame. */
            if (unstable) {
                fw_rest_guard_t discarded_fit;
                align_reference(layout, mean, reference, adjusted, &discarded_fit);
            }
        }
    }
    if (!unstable)
        return;
    for (unsigned row = 0; row < FW_MATRIX_ROWS; row++) {
        unsigned n = FW_ROW_LEN[layout][row];
        for (unsigned c = 0; c < n; c++) {
            unsigned s = FW_SENSOR_MAP[layout][row][c];
            if (!unstable[s])
                continue;
            if (g->match == FW_REST_MATCH)
                mean[s] = adc_range(adjusted[s]);
            else {
                if (c > 0)
                    g->key[row][c - 1] = FW_REST_UNCERTAIN;
                if (c + 1 < n)
                    g->key[row][c] = FW_REST_UNCERTAIN;
            }
        }
    }
}
