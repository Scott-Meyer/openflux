#include "fw_hdlc.h"

#include <string.h>

static uint8_t rev8(uint8_t b) {
    uint8_t r = 0;
    for (int i = 0; i < 8; i++) {
        r = (uint8_t)((r << 1) | ((b >> i) & 1U));
    }
    return r;
}

uint32_t fw_hdlc_crc32(const uint8_t *data, size_t len) {
    uint32_t crc = 0xFFFFFFFFU;
    for (size_t i = 0; i < len; i++) {
        crc ^= (uint32_t)rev8(data[i]) << 24;
        for (int bit = 0; bit < 8; bit++) {
            crc = (crc & 0x80000000U) ? ((crc << 1) ^ 0x04C11DB7U) : (crc << 1);
        }
    }
    return ~crc;
}

static int emit_byte(uint8_t b, uint8_t *out, size_t cap, size_t *pos) {
    if (b == FW_HDLC_FLAG || b == FW_HDLC_ESC) {
        if (*pos + 2U > cap)
            return -1;
        out[(*pos)++] = FW_HDLC_ESC;
        out[(*pos)++] = (uint8_t)(b ^ FW_HDLC_ESC_XOR);
    } else {
        if (*pos + 1U > cap)
            return -1;
        out[(*pos)++] = b;
    }
    return 0;
}

int fw_hdlc_encode(const uint8_t *payload, size_t len, uint8_t *out, size_t out_cap) {
    if (!out || out_cap < 7U || len > FW_HDLC_MAX_LEN || (len > 0U && !payload)) {
        return -1;
    }
    size_t pos = 0;
    out[pos++] = FW_HDLC_FLAG;
    if (emit_byte((uint8_t)((len >> 8) & 0xFFU), out, out_cap, &pos) < 0 ||
        emit_byte((uint8_t)(len & 0xFFU), out, out_cap, &pos) < 0) {
        return -1;
    }
    for (size_t i = 0; i < len; i++) {
        if (emit_byte(payload[i], out, out_cap, &pos) < 0)
            return -1;
    }
    uint32_t crc = fw_hdlc_crc32(payload, len);
    if (emit_byte((uint8_t)((crc >> 24) & 0xFFU), out, out_cap, &pos) < 0 ||
        emit_byte((uint8_t)((crc >> 16) & 0xFFU), out, out_cap, &pos) < 0 ||
        emit_byte((uint8_t)((crc >> 8) & 0xFFU), out, out_cap, &pos) < 0 ||
        emit_byte((uint8_t)(crc & 0xFFU), out, out_cap, &pos) < 0) {
        return -1;
    }
    return (int)pos;
}

void fw_hdlc_decoder_init(fw_hdlc_decoder_t *dec) {
    if (dec)
        memset(dec, 0, sizeof(*dec));
}

void fw_hdlc_decoder_feed(fw_hdlc_decoder_t *dec, const uint8_t *data, size_t len, fw_hdlc_cb_t cb, void *ctx) {
    if (!dec || (!data && len > 0U))
        return;
    for (size_t i = 0; i < len; i++) {
        uint8_t b = data[i];
        if (b == FW_HDLC_FLAG) {
            dec->state = 1;
            dec->escaped = 0;
            dec->expected_len = 0;
            dec->buf_len = 0;
            continue;
        }
        if (dec->state == 0)
            continue;
        if (dec->escaped) {
            b = (uint8_t)(b ^ FW_HDLC_ESC_XOR);
            dec->escaped = 0;
        } else if (b == FW_HDLC_ESC) {
            dec->escaped = 1;
            continue;
        }
        if (dec->state == 1) {
            dec->expected_len = (uint16_t)((uint16_t)b << 8);
            dec->state = 2;
        } else if (dec->state == 2) {
            dec->expected_len |= (uint16_t)b;
            if (dec->expected_len > FW_HDLC_MAX_LEN) {
                dec->state = 0;
                continue;
            }
            dec->buf_len = 0;
            dec->state = 3;
        } else if (dec->state == 3) {
            dec->buf[dec->buf_len++] = b;
            if (dec->buf_len == (size_t)dec->expected_len + 4U) {
                size_t plen = dec->expected_len;
                uint32_t rx_crc = ((uint32_t)dec->buf[plen] << 24) | ((uint32_t)dec->buf[plen + 1U] << 16) |
                                  ((uint32_t)dec->buf[plen + 2U] << 8) | (uint32_t)dec->buf[plen + 3U];
                if (rx_crc == fw_hdlc_crc32(dec->buf, plen)) {
                    if (cb)
                        cb(dec->buf, plen, ctx);
                } else {
                    dec->crc_errors++;
                }
                dec->state = 0;
                dec->buf_len = 0;
            }
        }
    }
}
