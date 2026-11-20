#ifndef FW_CLASSIC_ROW_H
#define FW_CLASSIC_ROW_H

#include <stdint.h>

/* Row conditioning for the model-free key tracker, used when press refinement is not active. */

/* Estimate the travel between consecutive Hall sensors in a row. Inputs are the
 * signed, calibrated ADC deltas in physical left-to-right order (whole counts).
 * row is 0..5, width is 2..17. The conditioned field has width entries and may
 * replace sensors in place; flux is a separate array of width−1 signed values,
 * before feel scaling and neighboring-key correction. There is no row history. */
void fw_classic_row(unsigned row, unsigned width, const int32_t *sensors, int32_t *conditioned, int32_t *flux);

#endif
