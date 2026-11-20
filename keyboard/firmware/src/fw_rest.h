#ifndef FW_REST_H
#define FW_REST_H

#include <stdbool.h>
#include <stdint.h>

#include "fw_protocol.h"

#define FW_REST_CHANNELS 112U
#define FW_REST_BYTES    236U

/* Integer fields in the record are little endian. Decode also accepts the byte-wise checksum written by
 * OpenFlux development builds (h = h * 31 + byte over bytes 0..231, starting at zero); encode never writes it.
 * Neither routine performs I/O or changes a stored record. */
int fw_rest_encode(uint8_t layout, uint8_t feel, const int16_t reference[FW_REST_CHANNELS], uint8_t out[FW_REST_BYTES]);
int fw_rest_decode(const uint8_t record[FW_REST_BYTES], uint8_t layout, uint8_t feel, int16_t out[FW_REST_CHANNELS]);

typedef enum { FW_REST_NO_REFERENCE, FW_REST_MATCH, FW_REST_OTHER_FRAME } fw_rest_match_t;
typedef enum { FW_REST_CLEAR, FW_REST_HELD, FW_REST_UNCERTAIN } fw_rest_key_t;

typedef struct {
    fw_rest_match_t match;
    int32_t thermal_gain_q10, thermal_offset;
    bool thermal_shift;
    uint8_t off_keys;   /* count before rejecting a reference from another frame */
    uint8_t key[6][16]; /* fw_rest_key_t, indexed by physical row and key in the row */
} fw_rest_guard_t;

/* Derive per-session baselines from a full eight-sweep acquisition. mean contains all 160
 * channels; unstable marks channels whose range across those samples was >40. Reference
 * is nullable and belongs to the detected (layout, feel). Only mean and guard change.
 * Substituted baselines are bounded to the ADC range. No record should be saved here. */
void fw_rest_apply(uint8_t layout, uint16_t mean[FW_NUM_SENSORS], const uint8_t unstable[FW_REST_CHANNELS],
                   const int16_t reference[FW_REST_CHANNELS], fw_rest_guard_t *guard);

#endif
