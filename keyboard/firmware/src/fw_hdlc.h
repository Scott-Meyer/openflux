#ifndef FW_HDLC_H
#define FW_HDLC_H

#include <stddef.h>
#include <stdint.h>

#define FW_HDLC_FLAG    0x7EU
#define FW_HDLC_ESC     0x7DU
#define FW_HDLC_ESC_XOR 0x20U
#define FW_HDLC_MAX_LEN 512U

typedef void (*fw_hdlc_cb_t)(const uint8_t *payload, size_t len, void *ctx);

typedef struct {
    uint8_t state;
    uint8_t escaped;
    uint16_t expected_len;
    size_t buf_len;
    uint32_t crc_errors;
    uint8_t buf[FW_HDLC_MAX_LEN + 4U];
} fw_hdlc_decoder_t;

uint32_t fw_hdlc_crc32(const uint8_t *data, size_t len);
int fw_hdlc_encode(const uint8_t *payload, size_t len, uint8_t *out, size_t out_cap);
void fw_hdlc_decoder_init(fw_hdlc_decoder_t *dec);
void fw_hdlc_decoder_feed(fw_hdlc_decoder_t *dec, const uint8_t *data, size_t len, fw_hdlc_cb_t cb, void *ctx);

#endif /* FW_HDLC_H */
