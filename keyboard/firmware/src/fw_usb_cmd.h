#ifndef FW_USB_CMD_H
#define FW_USB_CMD_H

#include "fw_protocol.h"
#include "fw_refinement.h"
#include "fw_scanner.h"

#include <stddef.h>
#include <stdint.h>

typedef int (*fw_usb_send_fn_t)(const uint8_t *pkt64, size_t len, void *ctx);

typedef struct {
    void (*on_mcu_bootloader)(void *ctx);
    void (*on_soc_bootloader)(void *ctx);
    int (*on_keymap_saved)(const uint8_t *blob, size_t len, void *ctx);
    int (*on_profile_settings_saved)(const uint8_t *xfer, size_t len, void *ctx);
    int (*on_language_saved)(uint32_t language_code, void *ctx);
    int (*on_platform_saved)(uint32_t platform_code, void *ctx);
    void *ctx;
    /* In enabled builds only: a complete record is durably installed; payload is borrowed for this call. */
    void (*on_refinement_installed)(const uint8_t payload[FW_REFINEMENT_PAYLOAD_BYTES], void *ctx);
    bool (*refinement_active)(void *ctx);
    /* Dynamic XZ-compressed vial.json uploaded by keyglassd (RAM-buffered and debounced to LittleFS). */
    int (*on_vial_def_uploaded)(const uint8_t *xz_data, size_t len, void *ctx);
    /* Analog gamepad config (0x74) and App-Linking layer override (0x75). */
    int (*on_gamepad_config)(const uint8_t *data, size_t len, void *ctx);
    int (*on_layer_override)(uint8_t layer, void *ctx);
} fw_usb_hooks_t;

typedef enum {
    FW_RX_IDLE = 0,
    FW_RX_RECEIVING = 1,
    FW_RX_DISCARDING = 2,
} fw_rx_state_t;

typedef struct {
    fw_rx_state_t state;
    uint8_t cmd;
    uint32_t expected_packets;
    uint32_t expected_bytes;
    uint32_t received_packets;
    uint32_t received_bytes;
    uint8_t rx_buf[FW_MAX_KEYMAP_BYTES];
    fw_runtime_t *rt;
    fw_usb_hooks_t hooks;
} fw_usb_rx_t;

typedef struct {
    uint8_t active;
    uint8_t hdr_sent;
    uint8_t progress_started;
    uint8_t hdr[FW_USB_PACKET_SIZE];
    const uint8_t *payload;
    uint32_t payload_len;
    uint32_t payload_off;
    uint32_t last_progress_ms;
} fw_usb_tx_t;

typedef uint32_t (*fw_usb_tx_avail_fn_t)(void *ctx);
typedef uint32_t (*fw_usb_tx_write_fn_t)(const uint8_t *pkt64, uint32_t len, void *ctx);
typedef void (*fw_usb_tx_flush_fn_t)(void *ctx);

/* A command whose packets stop arriving for this long is dropped (see fw_usb_rx_reset). */
#define FW_USB_RX_IDLE_TIMEOUT_MS 1000U
/* Backpressured TX with zero forward progress for this long is dropped so future commands cannot wedge. */
#define FW_USB_TX_STALL_TIMEOUT_MS 2000U

void fw_usb_rx_init(fw_usb_rx_t *rx, fw_runtime_t *rt, const fw_usb_hooks_t *hooks);
void fw_usb_tx_init(fw_usb_tx_t *tx);
void fw_usb_tx_reset(fw_usb_tx_t *tx);
int fw_usb_tx_busy(const fw_usb_tx_t *tx);
int fw_usb_tx_queue(fw_usb_tx_t *tx, uint8_t resp_type, const uint8_t *data, uint32_t len, uint8_t *backing_buf,
                    uint32_t backing_cap);
/* Feeds one packet and queues any response into `tx` for non-blocking chunked sending. */
void fw_usb_rx_feed_async(fw_usb_rx_t *rx, fw_usb_tx_t *tx, const uint8_t *data, size_t len);
/* Sends up to `max_packets` 64-byte packets without waiting on endpoint backpressure. Returns packets written. */
uint32_t fw_usb_tx_step(fw_usb_tx_t *tx, uint32_t now_ms, uint32_t max_packets, fw_usb_tx_avail_fn_t avail_fn,
                        fw_usb_tx_write_fn_t write_fn, fw_usb_tx_flush_fn_t flush_fn, void *ctx);
/* Feeds one packet read from the vendor OUT endpoint (synchronous callback helper). */
void fw_usb_rx_feed(fw_usb_rx_t *rx, const uint8_t *data, size_t len, fw_usb_send_fn_t send_fn, void *send_ctx);
/* Drops a partly received command without answering it, for a USB reset or a stalled transfer. */
void fw_usb_rx_reset(fw_usb_rx_t *rx);
/* Whether a command is partly received. */
int fw_usb_rx_busy(const fw_usb_rx_t *rx);

#endif /* FW_USB_CMD_H */
