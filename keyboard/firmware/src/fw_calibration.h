#ifndef FW_CALIBRATION_H
#define FW_CALIBRATION_H

#include "fw_protocol.h"

#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint8_t loaded_from_flash;
    uint32_t version;
    uint16_t factors[FW_NUM_SENSORS];
} fw_calibration_t;

void fw_calibration_init_defaults(fw_calibration_t *cal);

/* Validates factory_calibration_info (20 B) and factory_scaling_factors (320 B), then stores one Q8 factor per
 * sensor (0x0100 is 1.0). Version 2 factors are multiplied by 0xAA / 0x100 before use; other versions are used
 * as stored (docs/protocol/calibration-storage.md). Returns 0 on valid calibration, -1 on a validation or
 * checksum error, leaving the defaults intact. */
int fw_calibration_load(fw_calibration_t *cal, const uint8_t *info_bytes, size_t info_len, const uint8_t *factors_bytes,
                        size_t factors_len);

#endif /* FW_CALIBRATION_H */
