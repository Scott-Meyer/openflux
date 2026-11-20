#ifndef FW_REFINEMENT_H
#define FW_REFINEMENT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Press refinement runs in the scan loop only when FW_ENABLE_PRESS_REFINEMENT is 1 and valid data is installed.
 * Otherwise the scanner uses the model-free tracker and a valid record is only stored. */
#ifndef FW_ENABLE_PRESS_REFINEMENT
#define FW_ENABLE_PRESS_REFINEMENT 0
#endif

/* Serialized storage/USB envelope; payload bytes belong to the press-refinement engine. */
#define FW_REFINEMENT_FORMAT        1U
#define FW_REFINEMENT_HEADER_BYTES  16U
#define FW_REFINEMENT_PAYLOAD_BYTES 1456U
#define FW_REFINEMENT_RECORD_BYTES  (FW_REFINEMENT_HEADER_BYTES + FW_REFINEMENT_PAYLOAD_BYTES)

typedef enum {
    FW_REFINEMENT_ABSENT = 0,
    FW_REFINEMENT_INSTALLED = 1,
    FW_REFINEMENT_INVALID = 2,
    FW_REFINEMENT_UNAVAILABLE = 3,
} fw_refinement_state_t;

/* USB status for a valid stored record that the scanner is not applying. */
#define FW_REFINEMENT_STORED_INACTIVE 4U

/* Accept only an entire supported record, including its payload CRC. */
bool fw_refinement_validate(const uint8_t *record, size_t len);

#endif
