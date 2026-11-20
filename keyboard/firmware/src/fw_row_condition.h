#ifndef FW_ROW_CONDITION_H
#define FW_ROW_CONDITION_H

#include "fw_scanner.h" /* FW_MATRIX_MAX_COLS */

/* Remove the slowly varying spatial component of one row before reconstructing keys.
 * Input is signed calibrated sensor deltas in physical left-to-right order; the output retains four fractional bits.
 * A local magnetic peak is not evidence of an offset shared by the row. */
static inline void fw_condition_row_q4(int32_t row[], uint8_t n) {
    if (n == 0 || n > FW_MATRIX_MAX_COLS)
        return;

    int32_t sorted[FW_MATRIX_MAX_COLS], fit[FW_MATRIX_MAX_COLS];
    for (int i = 0; i < n; i++) {
        int j = i;
        while (j > 0 && sorted[j - 1] > row[i]) {
            sorted[j] = sorted[j - 1];
            j--;
        }
        sorted[j] = row[i];
    }
    if (sorted[0] == sorted[n - 1]) {
        for (int i = 0; i < n; i++)
            row[i] = 0;
        return;
    }

    /* Keep four fractional bits in the fit so an even-length row retains its half-count median. */
    int32_t median_q4 = (sorted[(n - 1) / 2] + sorted[n / 2]) * 8;
    for (int i = 0; i < n; i++)
        fit[i] = median_q4;

    uint32_t prior_inliers = 0;
    for (int pass = 0; pass < 2; pass++) {
        uint32_t inliers = 0;
        int count = 0, sum_x = 0, sum_xx = 0;
        int64_t sum_y = 0, sum_xy = 0;
        for (int i = 0; i < n; i++) {
            int32_t y = row[i] * 16;
            int32_t residual = y - fit[i];
            if (residual <= -100 * 16 || residual >= 100 * 16)
                continue;
            int x = 2 * i - (n - 1);
            inliers |= UINT32_C(1) << i;
            count++;
            sum_x += x;
            sum_xx += x * x;
            sum_y += y;
            sum_xy += (int64_t)x * y;
        }
        if (count < 4 || inliers == prior_inliers)
            break;
        prior_inliers = inliers;

        /* Least-squares line, evaluated about the mean inlier position. Slope and position have
         * 16 fractional bits each, so their product has 32; shifting it right by 32 gives whole
         * counts, rounded toward negative infinity. */
        int denominator = count * sum_xx - sum_x * sum_x; /* positive: at least four distinct positions */
        int64_t slope_q16 = ((count * sum_xy - sum_x * sum_y) * 65536) / denominator;
        int32_t mean_y = (int32_t)(sum_y / count);
        int32_t mean_x_q16 = sum_x * 65536 / count;
        for (int i = 0; i < n; i++) {
            int x = 2 * i - (n - 1);
            fit[i] = mean_y + (int32_t)((slope_q16 * (x * 65536 - mean_x_q16)) >> 32);
        }
    }
    for (int i = 0; i < n; i++)
        row[i] = row[i] * 16 - fit[i];
}

#endif /* FW_ROW_CONDITION_H */
