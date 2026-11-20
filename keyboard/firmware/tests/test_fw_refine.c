#include "fw_refine.h"

#include <assert.h>
#include <string.h>

static void test_quiet_release(void) {
    /* The left key's travel estimate depends on its right neighbor. Both keys
     * are released together, but the right one's shorter envelope expires first. */
    fw_refine_model_t model = {0};
    model.input_weights[11][12] = 1 << 13;
    model.input_bias[6] = -(1 << 23);
    model.output_weights[0] = 1 << 11;
    model.output_bias = 1 << 19;
    fw_refine_row_t row = {0};
    int32_t field[3] = {0}, depth[2] = {1000, 500}, out[2];
    for (int i = 0; i < 3; ++i)
        fw_refine_step(&model, &row, field, depth, 3, out);
    depth[0] = depth[1] = 0;
    for (int i = 0; i < 20; ++i)
        fw_refine_step(&model, &row, field, depth, 3, out);
    int32_t left = out[0];
    assert(left > 0 && out[1] > 0);
    for (int i = 0; i < 20; ++i) {
        fw_refine_step(&model, &row, field, depth, 3, out);
        assert(out[0] == left); /* Expiration is not fresh movement of the neighboring key. */
    }
    assert(out[1] == 0);
    field[1] = 80;
    fw_refine_step(&model, &row, field, depth, 3, out);
    assert(out[0] < left); /* A new change in the field wakes the estimate. */
}

void test_fw_refine(void) {
    test_quiet_release();
    /* Synthetic model: a positive correction at the left edge, a negative one at the right.
     * Give all three keys identical motion so the difference can only come from the loaded model. */
    uint8_t payload[FW_REFINE_MODEL_BYTES] = {0};
    const size_t input_bias = 6 * 2 + 18 * 4 + 18 * 6 * 2;
    const size_t input_weights = input_bias + 18 * 4;
    payload[0] = 0xFF; /* output weight for hidden channel 0 = +32767 */
    payload[1] = 0x7F;
    payload[input_bias + 4 * 6 + 3] = 0x80;                 /* update gate 0: prefer the candidate over prior state */
    const size_t edge = input_weights + 2 * (12 * 30 + 12); /* candidate 0, left and right edge flags */
    payload[edge] = 0xFF;
    payload[edge + 1] = 0x7F;
    payload[edge + 3] = 0x80;
    fw_refine_model_t model;
    assert(fw_refine_decode(&model, payload, sizeof(payload)) == 0);

    /* A truncated replacement must not damage the loaded model. */
    memset(payload, 0, sizeof(payload));
    assert(fw_refine_decode(&model, payload, sizeof(payload) - 1) != 0);
    fw_refine_row_t row = {0};
    int32_t residual[4] = {0};
    int32_t travel[3] = {600, 600, 600};
    fw_refine_step(&model, &row, residual, travel, 4, travel);
    assert(travel[0] == 999 && travel[1] == 600 && travel[2] == 200);
}
