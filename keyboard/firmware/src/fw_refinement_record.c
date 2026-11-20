#include "fw_refinement.h"

#include <string.h>

static uint32_t le32(const uint8_t *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

bool fw_refinement_validate(const uint8_t *record, size_t len) {
    if (!record || len != FW_REFINEMENT_RECORD_BYTES || memcmp(record, "OFRM", 4) != 0 ||
        le32(record + 4) != FW_REFINEMENT_FORMAT || le32(record + 8) != FW_REFINEMENT_PAYLOAD_BYTES)
        return false;

    uint32_t crc = UINT32_MAX;
    for (size_t i = FW_REFINEMENT_HEADER_BYTES; i < len; i++) {
        crc ^= record[i];
        for (unsigned bit = 0; bit < 8; bit++)
            crc = (crc >> 1) ^ ((crc & 1U) ? UINT32_C(0xedb88320) : 0U);
    }
    return ~crc == le32(record + 12);
}
