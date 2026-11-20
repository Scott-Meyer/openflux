#include "fw_depth.h"
#include "fw_scan_hot.h"

#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

/* A localized positive field has two opposite-signed lobes. Compare uncompensated
 * field to the area under the positive, centered integral for this run of keys. */
static bool balanced(const int32_t q[], unsigned lo, unsigned hi) {
    int32_t sum = 0;
    for (unsigned i = lo; i <= hi + 1; ++i)
        sum += q[i];
    int32_t center = sum / 2, integral = 0;
    uint32_t area = 0;
    for (unsigned i = lo; i <= hi; ++i) {
        integral += q[i];
        if (integral > center)
            area += (uint32_t)(integral - center);
    }
    /* area is an unsigned sum that is floored and compared as int32_t, like the tracker's other fixed-point
     * accumulators; a sum above INT32_MAX therefore takes the floor of 16. */
    if ((int32_t)area < 16)
        area = 16;
    return (int32_t)(100U * (uint32_t)abs(center)) <= (int32_t)(35U * area);
}

/* A strong edge can implicate several keys. Bridge the gap to the next strong
 * run only with evidence of existing motion in the gap, unless the gap is
 * quiet and both sides already account locally for their magnetic fields. */
static uint32_t depth_candidates(unsigned keys, const int32_t q[], const int32_t prior[]) {
    uint32_t candidates = 0;
    unsigned prior_lo = 0, prior_hi = 0;
    bool first = true;
    for (unsigned i = 0; i < keys;) {
        if (q[i] <= 1600 && q[i + 1] >= -1600) {
            ++i;
            continue;
        }
        unsigned lo = i;
        do {
            ++i;
        } while (i < keys && (q[i] > 1600 || q[i + 1] < -1600));
        unsigned hi = i - 1;
        bool merge = false;
        if (!first) {
            bool quiet = true;
            for (unsigned j = prior_hi + 1; j < lo; ++j) {
                if (prior[j] >= 1280)
                    merge = true;
                if (abs(q[j + 1]) >= 640)
                    quiet = false;
            }
            if (merge && quiet && balanced(q, prior_lo, prior_hi) && balanced(q, lo, hi))
                merge = false;
        }
        if (!merge)
            prior_lo = lo;
        prior_hi = hi;
        for (unsigned j = prior_lo; j <= hi; ++j)
            candidates |= (uint32_t)1 << j;
        first = false;
    }
    return candidates;
}

/* Residual = measured field − (this key's north lobe − preceding key's south lobe).
 * Endpoint sensors have only one neighbor. All of these fields remain in Q4. */
static void depth_residual(unsigned keys, const int32_t q[], const int32_t left[], const int32_t right[],
                           int32_t resid[]) {
    for (unsigned i = 0; i <= keys; ++i)
        resid[i] = q[i] - (i < keys ? left[i] : 0) + (i ? right[i - 1] : 0);
}

/* Q8 share of field. The multiply is done in uint32_t, so a product beyond int32_t wraps modulo 2^32
 * (defined behaviour) instead of overflowing; the signed result is then shifted. Host tests and the
 * RP2040 compute the same value. */
static int32_t share_q8(int32_t field, uint32_t weight) {
    return (int32_t)((uint32_t)field * (uint32_t)weight) >> 8;
}

/* Attribute each sensor's residual to the adjacent lobes proportionally to the
 * new evidence. With no new evidence favor the larger existing lobe pair.
 * Fractional remainders in *each* correction round down, even when negative. */
static void depth_update_lobes(unsigned keys, const int32_t resid[], const int32_t evidence[], int32_t left[],
                               int32_t right[]) {
    for (unsigned i = 0; i <= keys; ++i) {
        int32_t a = i < keys ? evidence[i] : 0, b = i ? evidence[i - 1] : 0;
        if (!a && !b) {
            a = i < keys ? left[i] + right[i] + 1 : 0;
            b = i ? left[i - 1] + right[i - 1] + 1 : 0;
        }
        /* The accumulator is a signed 32-bit sum. Treat a wrapped or nonpositive
         * total as unusable; only divide meaningful weights. */
        int32_t total = (int32_t)((uint32_t)a + (uint32_t)b), w = 0;
        /* An adjacent lobe can still be negative mid-sweep. Do not clamp a
         * signed fallback weight until both lobes have received their corrections. */
        if (total > 0) {
            if (a >= INT32_MIN / 256 && a <= INT32_MAX / 256)
                w = a * 256 / total;
            else
                w = (int32_t)((int64_t)a * 256 / total);
        }
        if (i < keys)
            left[i] += share_q8(resid[i], (uint32_t)w);
        if (i)
            right[i - 1] -= share_q8(resid[i], 256U - (uint32_t)w);
    }
    for (unsigned i = 0; i < keys; ++i) {
        int32_t a = left[i] > 0 ? left[i] : 0, b = right[i] > 0 ? right[i] : 0;
        int32_t mean = (a + b) / 2, bias = (a - b) / 2;
        left[i] = mean + bias;
        right[i] = mean - bias;
    }
}

/* Tridiagonal nearest-neighbor solve used to distribute coupled-row residuals.
 * Eight forward Gauss-Seidel sweeps; boundary sensor has one less neighbor. */
static void FW_SCAN_CODE(depth_decouple)(unsigned n, const int32_t signal[], int32_t estimate[]) {
    for (unsigned i = 0; i < n; ++i)
        estimate[i] = 0;
    for (int it = 0; it < 8; ++it)
        for (unsigned i = 0; i < n; ++i) {
            int neighbors = (i > 0) + (i + 1 < n);
            int32_t rest = (i ? estimate[i - 1] : 0) + (i + 1 < n ? estimate[i + 1] : 0);
            estimate[i] = (signal[i] - rest) / (5 + neighbors);
        }
}

/* Floor of the geometric mean without float; the inputs are in whole calibrated counts. */
static uint32_t isqrt32(uint32_t n) {
    uint32_t lo = 0, hi = 65535;
    while (lo < hi) {
        uint32_t mid = lo + (hi - lo + 1) / 2;
        if (mid * mid <= n)
            lo = mid;
        else
            hi = mid - 1;
    }
    return lo;
}

/* Per-key evidence from two adjacent opposite-polarity sensor residuals. Independent
 * four-sensor history windows allow distinct history for each key, including edges.
 * `suppress_geom` applies only to the geometric component under neighboring-key ambiguity. */
static int32_t depth_score(unsigned k, unsigned keys, const int32_t q[], const int32_t residual[], const int32_t left[],
                           const int32_t right[], const int16_t recent[4], bool suppress_geom) {
    const int32_t d = left[k] + right[k];
    const int32_t l = residual[k] / 16, r = residual[k + 1] / 16;
    int32_t floor = 4;
    bool localized = (q[k] / 16 - recent[1] > 10 && q[k + 1] / 16 - recent[2] < -10) &&
                     (k == 0 || abs(q[k - 1] / 16 - recent[0]) <= 10) &&
                     (k + 1 == keys || abs(q[k + 2] / 16 - recent[3]) <= 10);
    if (d >= 2400 || localized)
        floor = 1;
    int32_t score = 0;
    uint32_t opposed_product = 0U - (uint32_t)l * (uint32_t)r;
    if (!suppress_geom && abs(l) > floor && abs(r) > floor && opposed_product <= INT32_MAX)
        score = (int32_t)(16 * isqrt32(opposed_product));
    if (d < 480) {
        if (k == 0 || right[k - 1] < 480) {
            if (residual[k] > 0)
                score += residual[k];
        }
        if (k + 1 == keys || left[k + 1] < 480) {
            if (residual[k + 1] < 0)
                score -= residual[k + 1];
        }
    }
    return score;
}

/* Is the key too far below its earlier field strength for that position to
 * corroborate the current residual pattern? Both readings use the same scale. */
static bool fading(int j, int keys, const int32_t l[], const int32_t r[], const int32_t drop[], const int32_t peak[]) {
    return j >= 0 && j < keys && drop[j] > 16 && 100 * ((int64_t)l[j] + r[j]) < 80 * (int64_t)peak[j];
}

/* Each side of a weak key between two much deeper ones can: independently
 * support the key; say nothing; or look like a releasing neighbor. When the
 * latter pattern is visible on only one sensor, require corroboration. */
enum side_evidence { NEUTRAL, SUPPORT, WEAK_RELEASE, RELEASE, PAIRED_RELEASE };
static enum side_evidence FW_SCAN_CODE(flank)(int k, int dir, int keys, const int32_t err[], const int32_t l[],
                                              const int32_t r[], const int32_t drop[], const int32_t peak[]) {
    int inner = k + (dir == 1), polarity = -dir;
    int outer = err[inner + dir] * polarity;
    if (outer > 32 && !fading(k + 2 * dir, keys, l, r, drop, peak))
        return SUPPORT;
    if (outer >= -32)
        return NEUTRAL;
    int far_sensor = inner + 2 * dir;
    if (far_sensor >= 0 && far_sensor <= keys && err[far_sensor] * polarity > 32 &&
        !fading(k + 3 * dir, keys, l, r, drop, peak))
        return NEUTRAL;
    if (drop[k + dir] <= 16)
        return WEAK_RELEASE;
    return err[inner] * polarity > 32 ? PAIRED_RELEASE : RELEASE;
}
static int32_t key_strength(unsigned i, const int32_t l[], const int32_t r[], const int32_t prior[]) {
    int32_t d = (l[i] + r[i]) / 16;
    return d > prior[i] ? d : prior[i];
}
static bool depth_ambiguous(unsigned k, unsigned keys, const int32_t residual[], const int32_t left[],
                            const int32_t right[], const int32_t drop[], const int32_t prior[], const int32_t peak[]) {
    if (k == 0 || k + 1 == keys)
        return false;
    int32_t d = key_strength(k, left, right, prior);
    if (key_strength(k - 1, left, right, prior) <= d * 2 || key_strength(k + 1, left, right, prior) <= d * 2)
        return false;
    enum side_evidence a = flank((int)k, -1, (int)keys, residual, left, right, drop, peak),
                       b = flank((int)k, 1, (int)keys, residual, left, right, drop, peak);
    if (a == SUPPORT || b == SUPPORT)
        return false;
    if (a == PAIRED_RELEASE || b == PAIRED_RELEASE)
        return true;
    return (a == RELEASE && (b == RELEASE || b == WEAK_RELEASE)) || (b == RELEASE && a == WEAK_RELEASE);
}

/* Share residuals between neighbors, then repeat using the updated field estimate. */
static void fit_lobes(unsigned keys, const int32_t q[], fw_depth_row_t *state) {
    int32_t err[FW_DEPTH_SENSORS], score[FW_DEPTH_KEYS];
    const fw_depth_history_t *h = &state->history;
    int32_t *left = state->left, *right = state->right;
    for (unsigned pass = 0; pass < 3; ++pass) {
        depth_residual(keys, q, left, right, err);
        for (unsigned k = 0; k < keys; ++k) {
            bool veto = depth_ambiguous(k, keys, err, left, right, h->last_drop, h->extra_depth, h->group_peak);
            score[k] = depth_score(k, keys, q, err, left, right, h->rest_window[k], veto);
        }
        depth_update_lobes(keys, err, score, left, right);
    }
}

static int32_t mag(int32_t x) {
    return x < 0 ? -x : x;
}
static int32_t smaller(int32_t a, int32_t b) {
    return a < b ? a : b;
}
static int32_t larger(int32_t a, int32_t b) {
    return a > b ? a : b;
}
static bool candidate(uint32_t mask, int i) {
    return i >= 0 && (mask & (1u << i));
}
static uint8_t tick(uint8_t value) {
    return value < UINT8_MAX ? value + 1 : value;
}

static void settle(unsigned n, const int32_t q[], uint32_t mask, int32_t l[], int32_t r[], uint8_t *age) {
    for (unsigned i = 0; i < n; ++i) {
        age[i] = candidate(mask, (int)i) ? 0 : tick(age[i]);
        if (age[i] > 60) {
            l[i] -= l[i] >> 3;
            r[i] -= r[i] >> 3;
        }
    }
    for (int i = 0; i < (int)n; ++i) {
        if (candidate(mask, i - 1) || candidate(mask, i + 1))
            continue;
        if (!candidate(mask, (int)i)) {
            l[i] = r[i] = 0;
        } else if ((!i || l[i - 1] + r[i - 1] < 160) && (i + 1 == (int)n || l[i + 1] + r[i + 1] < 160)) {
            l[i] = larger(q[i], 0);
            r[i] = larger(-q[i + 1], 0);
        }
    }
}

static void learn_window(unsigned n, const int32_t q[], int32_t l[], int32_t r[], fw_depth_history_t *s) {
    for (unsigned i = 0; i < n; ++i) {
        int32_t z[4];
        for (int j = 0; j < 4; ++j) {
            int k = (int)i + j - 1;
            z[j] = (k < 0 || k > (int)n) ? 0 : q[k];
        }
        int sum = l[i] + r[i];
        int16_t *ref = s->rest_window[i];
        /* A just-emerging bipolar change at this key must not be learned as its new rest level. */
        bool emerging = sum < 480 && (!i || mag(z[0] / 16 - ref[0]) <= 10) &&
                        (i + 1 == n || mag(z[3] / 16 - ref[3]) <= 10) && z[1] / 16 - ref[1] > 1 &&
                        z[2] / 16 - ref[2] < -1;
        bool still = true, recovered = true;
        for (int j = 0; j < 4; ++j) {
            int k = (int)i + j - 1;
            int32_t v = z[j] / 16;
            bool quiet = !emerging && (k < 0 || k > (int)n || mag(v - s->prev_field[k]) <= 10);
            if (quiet && sum < 480)
                ref[j] = (int16_t)v;
            still &= quiet;
            recovered &= mag(v - ref[j]) <= 10;
        }
        s->stable_for[i] = still ? tick(s->stable_for[i]) : 0;
        if (sum < 480) {
            s->window_peak[i] = 0;
            continue;
        }
        s->window_peak[i] = larger(s->window_peak[i], sum);
        if (!recovered) {
            if (s->stable_for[i] < 8 || (int64_t)sum * 100 >= (int64_t)s->window_peak[i] * 20 || z[1] >= -1600 ||
                z[2] <= 1600 || (i && (int64_t)-z[1] * 100 < (int64_t)z[0] * 70) ||
                (i + 1 < n && (int64_t)z[2] * 100 < (int64_t)-z[3] * 70))
                continue;
            for (int j = 0; j < 4; ++j)
                ref[j] = (int16_t)(z[j] / 16);
        }
        /* Equal facing lobes at a shared sensor cancel in the measured magnetic field. */
        if (i) {
            int d = smaller(l[i], r[i - 1]);
            l[i] -= d;
            r[i - 1] -= d;
        }
        if (i + 1 < n) {
            int d = smaller(r[i], l[i + 1]);
            r[i] -= d;
            l[i + 1] -= d;
        }
    }
    for (unsigned i = 0; i <= n; ++i)
        s->prev_field[i] = (int16_t)(q[i] / 16);
}

static void lift_group(unsigned n, const int32_t x[], const int32_t lobes[], const int32_t peak[], int32_t out[]) {
    /* Compare the centered integral obtained by redistributing net row imbalance over active
     * sensors with one obtained by decoupling neighboring fields. Use the lower estimate. */
    int32_t sum = 0, abs_sum = 0;
    for (unsigned i = 0; i <= n; ++i) {
        sum += x[i];
        abs_sum += mag(x[i]);
    }
    if (!abs_sum)
        return;
    int32_t integrals[FW_DEPTH_KEYS], final = 0;
    for (unsigned i = 0; i <= n; ++i) {
        final += x[i] - (int32_t)((int64_t)sum * mag(x[i]) / abs_sum);
        if (i < n)
            integrals[i] = final;
    }
    int32_t rhs[FW_DEPTH_KEYS], offset[FW_DEPTH_KEYS], shape[FW_DEPTH_KEYS];
    for (unsigned i = 0; i < n; ++i)
        rhs[i] = (i ? x[i] : 0) + (i + 1 < n ? x[i + 1] : 0);
    depth_decouple(n, rhs, offset);
    for (unsigned i = 0; i < n; ++i)
        rhs[i] = 65536;
    depth_decouple(n, rhs, shape);
    int32_t error = sum / 2, norm = 0;
    for (unsigned i = 0; i < n; ++i) {
        error -= offset[i];
        norm += shape[i];
    }
    int32_t running = 0, prior = 0;
    for (unsigned i = 0; i < n; ++i) {
        int32_t correction = offset[i] + (int32_t)((int64_t)error * shape[i] / norm);
        running += x[i];
        int32_t f = smaller(integrals[i] - final / 2, running - correction - 2 * prior);
        prior += correction;
        /* Compare signed hundredths in the fixed-width field accumulators. */
        int32_t gradient = (x[i] - x[i + 1]) / 2;
        if (f <= 0 || (int32_t)((uint32_t)gradient * 100U) < (int32_t)(0U - (uint32_t)f * 100U) || lobes[i] < 1280 ||
            (int64_t)lobes[i] * 100 < (int64_t)peak[i] * 15)
            continue;
        int32_t raw = lobes[i] / 16, adjusted = f / 8;
        if (raw < 140 && adjusted > raw)
            adjusted = raw + (int32_t)((int64_t)(adjusted - raw) * (raw - 80) / 60);
        out[i] = larger(out[i], adjusted);
    }
}

static void depth_postfit(unsigned n, const int32_t q[], uint32_t mask, bool fitted, int32_t l[], int32_t r[],
                          fw_depth_history_t *s, int32_t out[]) {
    int32_t lobes[FW_DEPTH_KEYS];
    if (fitted) {
        settle(n, q, mask, l, r, s->skipped_for);
        learn_window(n, q, l, r, s);
    } else {
        for (unsigned i = 0; i <= n; ++i)
            s->prev_field[i] = (int16_t)(q[i] / 16);
        for (unsigned i = 0; i < n; ++i)
            for (int j = 0; j < 4; ++j) {
                int k = (int)i + j - 1;
                s->rest_window[i][j] = (k < 0 || k > (int)n) ? 0 : s->prev_field[k];
            }
    }
    for (unsigned i = 0; i < n; ++i) {
        lobes[i] = l[i] + r[i];
        if (fitted) {
            s->last_drop[i] = s->previous_sum[i] - lobes[i];
            s->previous_sum[i] = lobes[i];
        }
        bool pressed_region = candidate(mask, (int)i) && lobes[i] >= 1280;
        s->group_peak[i] = pressed_region ? larger(s->group_peak[i], lobes[i]) : 0;
        out[i] = lobes[i] / 16;
    }
    for (unsigned i = 0; i < n;) {
        if (!candidate(mask, (int)i)) {
            ++i;
            continue;
        }
        unsigned first = i++;
        while (i < n && candidate(mask, (int)i))
            ++i;
        if (i - first > 1)
            lift_group(i - first, q + first, lobes + first, s->group_peak + first, out + first);
    }
    for (unsigned i = 0; i < n; ++i) {
        if (candidate(mask, (int)i) && lobes[i] >= 1280 && (int64_t)lobes[i] * 100 >= (int64_t)s->group_peak[i] * 15)
            out[i] = larger(out[i], s->extra_depth[i] - (s->extra_depth[i] >> 5));
        out[i] = larger(0, out[i]);
        s->extra_depth[i] = out[i] > lobes[i] / 16 ? out[i] : 0;
    }
}

void fw_depth_row_reset(fw_depth_row_t *state) {
    memset(state, 0, sizeof(*state));
}

void FW_SCAN_CODE(fw_depth_row_update)(fw_depth_row_t *state, unsigned keys, const int32_t q4[], int32_t depth[]) {
    if (keys == 0 || keys > FW_DEPTH_KEYS)
        return;
    int32_t previous[FW_DEPTH_KEYS];
    bool warm = false;
    for (unsigned k = 0; k < keys; k++) {
        previous[k] = state->left[k] + state->right[k];
        warm |= previous[k] != 0;
    }
    uint32_t region = depth_candidates(keys, q4, previous);
    bool fitted = warm || region != 0;
    if (fitted)
        fit_lobes(keys, q4, state);
    depth_postfit(keys, q4, region, fitted, state->left, state->right, &state->history, depth);
}
