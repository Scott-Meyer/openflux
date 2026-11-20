#ifndef FW_REST_INTERNAL_H
#define FW_REST_INTERNAL_H

#include <stdint.h>

/* A field consists of non-quiet sensors with gaps of at most three quiet slots. Inside each
 * field, travel is the accumulated signed signal to the left of a key minus half the total.
 * Labels are 1-based, zero for slots outside a field. */
static inline unsigned fw_rest_fields(const int32_t *delta, unsigned n, int32_t *travel, uint8_t *label) {
    unsigned id = 0;
    for (unsigned c = 0; c < n; c++) {
        travel[c] = 0;
        label[c] = 0;
    }
    for (unsigned first = 0; first < n;) {
        if (delta[first] >= -40 && delta[first] <= 40) {
            first++;
            continue;
        }
        unsigned last = first;
        for (unsigned c = first + 1; c < n && c <= last + 4; c++)
            if (delta[c] < -40 || delta[c] > 40)
                last = c;
        id++;
        int32_t total = 0, prefix = 0;
        for (unsigned c = first; c <= last; c++) {
            total += delta[c];
            label[c] = (uint8_t)id;
        }
        for (unsigned c = first; c < last; c++) {
            prefix += delta[c];
            travel[c] = prefix - total / 2;
        }
        first = last + 1;
    }
    return id;
}
#endif
