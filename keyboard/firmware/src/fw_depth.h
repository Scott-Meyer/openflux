#ifndef FW_DEPTH_H
#define FW_DEPTH_H

#include <stdint.h>

enum { FW_DEPTH_KEYS = 16, FW_DEPTH_SENSORS = 17 };
/* Persistent state for one physical row. All int32 quantities except extra_depth are Q4. */
typedef struct {
    uint8_t skipped_for[FW_DEPTH_KEYS];
    int16_t prev_field[FW_DEPTH_SENSORS];  /* integer signal units */
    int16_t rest_window[FW_DEPTH_KEYS][4]; /* integer signal units, offsets -1..+2 */
    uint8_t stable_for[FW_DEPTH_KEYS];
    int32_t window_peak[FW_DEPTH_KEYS];
    int32_t previous_sum[FW_DEPTH_KEYS];
    int32_t last_drop[FW_DEPTH_KEYS]; /* previous minus new lobe sum */
    int32_t group_peak[FW_DEPTH_KEYS];
    int32_t extra_depth[FW_DEPTH_KEYS]; /* integer signal units, retained peak estimate */
} fw_depth_history_t;

/* State of the spatial estimator on one physical row. Initialize to all zeroes
 * on startup or layout change; keep it across frames, including idle frames. */
typedef struct {
    int32_t left[FW_DEPTH_KEYS], right[FW_DEPTH_KEYS];
    fw_depth_history_t history;
} fw_depth_row_t;

void fw_depth_row_reset(fw_depth_row_t *state);

/* q4[0..keys] are the signed sensor residuals of the row-line fit (16 units per
 * calibrated count). Produce keys depth estimates in whole calibrated counts.
 * These estimates are input to the subsequent board-weighted tracking stage. */
void fw_depth_row_update(fw_depth_row_t *state, unsigned keys, const int32_t q4[], int32_t depth[]);
#endif
