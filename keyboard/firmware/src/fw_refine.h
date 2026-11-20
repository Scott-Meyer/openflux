#ifndef FW_REFINE_H
#define FW_REFINE_H

#include <stddef.h>
#include <stdint.h>

#define FW_REFINE_MODEL_BYTES 1456U
#define FW_REFINE_MAX_KEYS    16U

/* One immutable model shared by every row. The two matrices are input-major, so changing a
 * single feature can update all gate projections without traversing unrelated weights. */
typedef struct {
    int16_t input_weights[30][18], history_weights[6][18], output_weights[6];
    int32_t input_bias[18], history_bias[18], output_bias;
} fw_refine_model_t;

/* Decode a packed little-endian model into independent storage. Returns 0 on success, -1 on
 * a null argument or wrong length, leaving out unchanged. The buffers must not overlap.
 * Integrity checks belong to the loader; all coefficient bit patterns are safe to evaluate. */
int fw_refine_decode(fw_refine_model_t *out, const uint8_t *payload, size_t len);

typedef struct {
    int32_t input_sum[18], history_sum[18], score, flux, envelope;
    int16_t features[28], hidden[6], previous_hidden[6];
    uint8_t active;
} fw_refine_key_t;

typedef struct {
    fw_refine_key_t key[FW_REFINE_MAX_KEYS];
    int32_t previous_residual[FW_REFINE_MAX_KEYS + 1], previous_provisional[FW_REFINE_MAX_KEYS];
    uint8_t width, settled;
} fw_refine_row_t;

/* Advance one complete scan. residual has width (2..17) conditioned sensor signals in row
 * order, provisional has width-1 preliminary key fluxes; out receives width-1 refined fluxes
 * and may alias an input. The tracker needs every scan, including unchanged/idle readings.
 * Start row state at zero and zero it again after model, layout or calibration changes. */
void fw_refine_step(const fw_refine_model_t *model, fw_refine_row_t *row, const int32_t *residual,
                    const int32_t *provisional, unsigned width, int32_t *out);

#endif /* FW_REFINE_H */
