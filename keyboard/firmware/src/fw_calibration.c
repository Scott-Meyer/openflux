#include "fw_calibration.h"

#include <string.h>

/* Version 2 factors are used multiplied by 0xAA / 0x100. */
#define FW_CALIB_V2_SCALE_Q8 0xAAU

static uint32_t rd_le32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static uint16_t rd_le16(const uint8_t *p) {
    return (uint16_t)p[0] | ((uint16_t)p[1] << 8);
}

void fw_calibration_init_defaults(fw_calibration_t *cal) {
    if (!cal)
        return;
    memset(cal, 0, sizeof(*cal));
    for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
        cal->factors[i] = FW_CALIB_DEFAULT_Q8;
    }
}

int fw_calibration_load(fw_calibration_t *cal, const uint8_t *info_bytes, size_t info_len, const uint8_t *factors_bytes,
                        size_t factors_len) {
    if (!cal || !info_bytes || !factors_bytes || info_len != 20U || factors_len != (FW_NUM_SENSORS * 2U)) {
        return -1;
    }
    uint32_t magic = rd_le32(info_bytes + 0);
    uint32_t version = rd_le32(info_bytes + 4);
    uint32_t num_sensors = rd_le32(info_bytes + 8);
    uint32_t checksum = rd_le32(info_bytes + 16);
    if (magic != FW_CALIB_MAGIC || num_sensors != FW_NUM_SENSORS) {
        return -1;
    }

    uint16_t tmp[FW_NUM_SENSORS];
    uint32_t sum = 0;
    for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
        tmp[i] = rd_le16(factors_bytes + i * 2U);
        sum += tmp[i];
    }
    if (sum != checksum) {
        return -1;
    }

    for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
        cal->factors[i] = (version == 2U) ? (uint16_t)(((uint32_t)tmp[i] * FW_CALIB_V2_SCALE_Q8) >> 8) : tmp[i];
    }
    cal->version = version;
    cal->loaded_from_flash = 1U;
    return 0;
}
