#include "fw_refine.h"
#include "fw_scan_hot.h"
#include "fw_refine_lut.h"

#include <limits.h>
#include <string.h>

static int32_t signed_word(uint32_t bits) {
    return bits <= INT32_MAX ? (int32_t)bits : -1 - (int32_t)~bits;
}

static int16_t next_i16(const uint8_t **cursor) {
    const uint8_t *p = *cursor;
    int32_t bits = p[0] | ((int32_t)p[1] << 8);
    *cursor += 2;
    return (int16_t)(bits > INT16_MAX ? bits - 65536 : bits);
}

static int32_t next_i32(const uint8_t **cursor) {
    const uint8_t *p = *cursor;
    uint32_t bits = p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
    *cursor += 4;
    return signed_word(bits);
}

int fw_refine_decode(fw_refine_model_t *out, const uint8_t *payload, size_t len) {
    if (!out || !payload || len != FW_REFINE_MODEL_BYTES)
        return -1;
    /* File order is output projection, recurrent bias/weights, input bias/weights, output bias.
     * Each wire matrix is [gate][input]; transpose once when loading rather than every scan. */
    for (unsigned h = 0; h < 6; h++)
        out->output_weights[h] = next_i16(&payload);
    for (unsigned j = 0; j < 18; j++)
        out->history_bias[j] = next_i32(&payload);
    for (unsigned j = 0; j < 18; j++)
        for (unsigned h = 0; h < 6; h++)
            out->history_weights[h][j] = next_i16(&payload);
    for (unsigned j = 0; j < 18; j++)
        out->input_bias[j] = next_i32(&payload);
    for (unsigned j = 0; j < 18; j++)
        for (unsigned f = 0; f < 30; f++)
            out->input_weights[f][j] = next_i16(&payload);
    out->output_bias = next_i32(&payload);
    return 0;
}

static int32_t accumulate_product(int32_t c, int32_t a, int32_t b) {
    /* Accumulators are 32-bit rings. Unsigned arithmetic defines wrap on all C targets. */
    return signed_word((uint32_t)c + (uint32_t)a * (uint32_t)b);
}
static int32_t clip_feature(int64_t v) {
    return v < -32767 ? -32767 : v > 32767 ? 32767 : (int32_t)v;
}
static int32_t max32(int32_t a, int32_t b) {
    return a > b ? a : b;
}
static int32_t min32(int32_t a, int32_t b) {
    return a < b ? a : b;
}

static int32_t sample_activation(const int16_t table[513], int32_t input) {
    const int32_t one = 1 << 16;
    int32_t ix;
    if (input < -256 * one)
        ix = 0;
    else if (input >= 256 * one)
        ix = (512 * one) - 1;
    else
        ix = input + 256 * one;
    int j = ix >> 16, rem = ix & 65535;
    return table[j] + (((table[j + 1] - table[j]) * rem) >> 16);
}

static void init_row(const fw_refine_model_t *model, fw_refine_row_t *row, const int32_t *residual,
                     const int32_t *provisional, unsigned n) {
    memset(row, 0, sizeof *row);
    row->width = (uint8_t)(n + 1);
    memcpy(row->previous_residual, residual, (n + 1) * sizeof *residual);
    memcpy(row->previous_provisional, provisional, n * sizeof *provisional);
    for (unsigned k = 0; k < n; k++) {
        fw_refine_key_t *key = row->key + k;
        for (unsigned i = 0; i < 18; i++) {
            int32_t bias = model->input_bias[i];
            if (k == 0)
                bias = accumulate_product(bias, model->input_weights[12][i], 1024);
            if (k + 1 == n)
                bias = accumulate_product(bias, model->input_weights[13][i], 1024);
            key->input_sum[i] = bias;
            key->history_sum[i] = model->history_bias[i];
        }
    }
}

static int update_feature(fw_refine_key_t *key, const fw_refine_model_t *model, unsigned f, int value, int tolerance) {
    const int cache = key->features[f];
    if (value >= -tolerance && value <= tolerance)
        value = 0;
    else if (value - cache >= -tolerance && value - cache <= tolerance)
        return 0;
    const int delta = value - cache;
    if (delta == 0)
        return 0;
    key->features[f] = (int16_t)value;
    /* Input columns 12 and 13 are constant left/right edge flags, folded into the initial bias. */
    const int16_t *w = model->input_weights[f + (f >= 12 ? 2 : 0)];
    for (unsigned j = 0; j < 18; j++)
        key->input_sum[j] = accumulate_product(key->input_sum[j], delta, w[j]);
    return 1;
}

/* Six hidden states, reset/update gates followed by six candidate projections. */
static void gru_step(const fw_refine_model_t *model, fw_refine_key_t *k) {
    int32_t score = model->output_bias;
    for (unsigned i = 0; i < 6; i++) {
        int32_t reset = sample_activation(sigmoid_q15, accumulate_product(k->input_sum[i], 1, k->history_sum[i])) >> 5;
        int32_t retain =
            sample_activation(sigmoid_q15, accumulate_product(k->input_sum[6 + i], 1, k->history_sum[6 + i]));
        int32_t candidate = sample_activation(tanh_q15, accumulate_product(k->input_sum[12 + i], reset,
                                                                           k->history_sum[12 + i] >> 10)) >>
                            5;
        int32_t h = candidate + (((k->hidden[i] - candidate) * retain) >> 15);
        k->hidden[i] = (int16_t)clip_feature(h);
        score = accumulate_product(score, model->output_weights[i], k->hidden[i]);
    }
    k->score = score;
}

/* A settled row keeps its recurrent state while only its travel envelopes decay.
 * Check the field as well as the provisional travel: a coherent run of small
 * residuals can still produce a significant cumulative gradient. */
static int quiet_input(const fw_refine_row_t *row, const int32_t *residual, const int32_t *provisional, unsigned n) {
    int sum = 0;
    for (unsigned i = 0; i < n; ++i)
        if (provisional[i] < -16 || provisional[i] > 16 || provisional[i] != row->previous_provisional[i])
            return 0;
    for (unsigned i = 0; i <= n; ++i) {
        int32_t field = residual[i], previous = row->previous_residual[i];
        if (field < -16 || field > 16 || previous < field - 6 || previous > field + 6)
            return 0;
        sum += field;
    }
    int gradient = -sum;
    for (unsigned i = 0; i < n; ++i) {
        gradient += 2 * residual[i];
        if (gradient < -32 || gradient > 32)
            return 0;
    }
    return 1;
}

void FW_SCAN_CODE(fw_refine_step)(const fw_refine_model_t *model, fw_refine_row_t *row, const int32_t *residual,
                                  const int32_t *provisional, unsigned width, int32_t *out) {
    if (!model || !row || !residual || !provisional || !out || width < 2 || width > FW_REFINE_MAX_KEYS + 1U)
        return;
    const unsigned n = width - 1;
    if (width != row->width)
        init_row(model, row, residual, provisional, n);
    /* Pad both ends so the same neighborhood describes edge and interior keys. */
    int32_t value[FW_REFINE_MAX_KEYS + 4] = {0}, velocity[FW_REFINE_MAX_KEYS + 4] = {0};
    int32_t base[FW_REFINE_MAX_KEYS + 2] = {0}, previous_flux[FW_REFINE_MAX_KEYS + 2] = {0};
    int64_t sum = 0, prefix = 0;
    uint8_t advance[FW_REFINE_MAX_KEYS] = {0};
    for (unsigned i = 0; i < width; i++) {
        value[1 + i] = clip_feature(4LL * residual[i]);
        velocity[1 + i] = clip_feature(32LL * ((int64_t)residual[i] - row->previous_residual[i]));
        sum += residual[i];
    }
    int engaged = 0;
    for (unsigned i = 0; i < n; i++) {
        base[i + 1] = clip_feature(provisional[i]);
        previous_flux[i + 1] = clip_feature(row->key[i].flux);
        engaged |= provisional[i] > 20 || row->key[i].active || row->key[i].envelope >= 400;
    }
    int quiet = engaged && quiet_input(row, residual, provisional, n);
    int evaluate = engaged && !(quiet && row->settled);
    int any_change = 0;
    for (unsigned i = 0; evaluate && i < n; i++) {
        fw_refine_key_t *key = row->key + i;
        prefix += residual[i];
        /* The recurrent step is unnecessary for isolated travel at rest. */
        int active = max32(base[i], max32(base[i + 1], base[i + 2])) > 20 || key->envelope >= 400;
        int changed = active != key->active;
        key->active = (uint8_t)active;
        if (!active)
            continue;
        /* Values and velocities from the two sensors bordering the key and their outside neighbors;
         * then prior and provisional travel of this key and adjacent keys. */
        const int32_t neighborhood[16] = {
            value[i + 1],
            value[i + 2],
            value[i],
            value[i + 3],
            clip_feature(2 * (2 * prefix - sum)),
            velocity[i + 1],
            velocity[i + 2],
            velocity[i],
            velocity[i + 3],
            previous_flux[i],
            previous_flux[i + 1],
            previous_flux[i + 2],
            base[i + 1],
            base[i],
            base[i + 2],
            clip_feature(16LL * ((int64_t)provisional[i] - row->previous_provisional[i]))};
        static const int16_t FW_SCAN_TABLE(tolerance)[16] = {64,  64, 64, 64, 64, 192, 192, 192,
                                                             192, 16, 16, 16, 16, 16,  16,  256};
        for (unsigned f = 0; f < 16; f++)
            changed |= update_feature(key, model, f, neighborhood[f], tolerance[f]);
        for (unsigned h = 0; h < 6; h++) {
            changed |= update_feature(key, model, 16 + h, i ? row->key[i - 1].hidden[h] : 0, 0);
            changed |= update_feature(key, model, 22 + h, i + 1 < n ? row->key[i + 1].hidden[h] : 0, 0);
            int delta = key->hidden[h] - key->previous_hidden[h];
            if (delta) {
                for (unsigned j = 0; j < 18; j++)
                    key->history_sum[j] = accumulate_product(key->history_sum[j], delta, model->history_weights[h][j]);
                changed = 1;
                key->previous_hidden[h] = key->hidden[h];
            }
        }
        advance[i] = (uint8_t)changed;
        any_change |= changed;
    }
    if (evaluate)
        row->settled = (uint8_t)(quiet && !any_change);
    /* All keys above read their neighbors' previous states. Only now install this scan's hidden states. */
    memcpy(row->previous_residual, residual, width * sizeof *residual);
    memcpy(row->previous_provisional, provisional, n * sizeof *provisional);
    for (unsigned i = 0; i < n; i++) {
        fw_refine_key_t *key = row->key + i;
        if (advance[i])
            gru_step(model, key);
        int32_t baseline = max32(0, row->previous_provisional[i]);
        key->envelope = max32(baseline, key->envelope - 4);
        int64_t cap = (int64_t)baseline + max32(baseline - 160, 0);
        int upper = max32(key->envelope >= 400 ? key->envelope : 0, cap > INT32_MAX ? INT32_MAX : (int32_t)cap);
        int delta = (25 * sample_activation(tanh_q15, key->score)) >> 11;
        int64_t adjusted = (int64_t)baseline + delta;
        key->flux = max32(0, min32(upper, adjusted > INT32_MAX ? INT32_MAX : (int32_t)adjusted));
        out[i] = key->flux;
    }
}
