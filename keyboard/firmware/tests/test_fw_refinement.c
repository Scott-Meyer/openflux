#include "fw_usb_cmd.h"
#include "lfs_store.h"

#include <assert.h>
#include <string.h>

static fw_usb_rx_t usb;
static uint8_t response[FW_USB_PACKET_SIZE + FW_REFINEMENT_RECORD_BYTES];
static size_t bytes_sent;
static unsigned completions, activations;
static uint8_t active_payload[FW_REFINEMENT_PAYLOAD_BYTES];

static void le32(uint8_t *out, uint32_t v) {
    for (unsigned i = 0; i < 4; i++)
        out[i] = (uint8_t)(v >> (8U * i));
}

static int collect(const uint8_t *packet, size_t n, void *ctx) {
    (void)ctx;
    if (n == 0) {
        completions++;
    } else {
        assert(n <= sizeof(response) - bytes_sent);
        memcpy(response + bytes_sent, packet, n);
        bytes_sent += n;
    }
    return 0;
}

static void activate(const uint8_t *payload, void *ctx) {
    (void)ctx;
    activations++;
    memcpy(active_payload, payload, sizeof(active_payload));
}

static bool refinement_active(void *ctx) {
    (void)ctx;
    return activations != 0;
}

static void expect_activation(unsigned commits, const uint8_t *record) {
    if (FW_ENABLE_PRESS_REFINEMENT) {
        assert(activations == commits);
        assert(memcmp(active_payload, record + FW_REFINEMENT_HEADER_BYTES, sizeof(active_payload)) == 0);
    } else {
        assert(activations == 0);
    }
}

static void request(uint8_t cmd, const uint8_t *payload, size_t len, size_t send_bytes) {
    uint8_t header[FW_USB_PACKET_SIZE] = {cmd};
    size_t pkts = (len + FW_USB_PACKET_SIZE - 1) / FW_USB_PACKET_SIZE;
    le32(header + 1, (uint32_t)pkts);
    le32(header + 5, (uint32_t)len);
    bytes_sent = completions = 0;
    fw_usb_rx_feed(&usb, header, sizeof(header), collect, NULL);
    while (send_bytes) {
        uint8_t page[FW_USB_PACKET_SIZE] = {0};
        size_t n = send_bytes < sizeof(page) ? send_bytes : sizeof(page);
        memcpy(page, payload, n);
        fw_usb_rx_feed(&usb, page, sizeof(page), collect, NULL);
        payload += n;
        send_bytes -= n;
    }
}

static void expect(uint8_t ack, const uint8_t *payload, size_t n) {
    size_t packets = (n + FW_USB_PACKET_SIZE - 1) / FW_USB_PACKET_SIZE;
    uint8_t header[9] = {ack};
    le32(header + 1, (uint32_t)packets);
    le32(header + 5, (uint32_t)n);
    assert(completions == 1 && bytes_sent == (1 + packets) * FW_USB_PACKET_SIZE);
    assert(memcmp(response, header, sizeof(header)) == 0);
    if (n)
        assert(memcmp(response + FW_USB_PACKET_SIZE, payload, n) == 0);
}

static void expect_status(fw_refinement_state_t state, uint32_t crc) {
    uint8_t status[16] = {1};
    le32(status + 4, state == FW_REFINEMENT_INSTALLED && !FW_ENABLE_PRESS_REFINEMENT ? FW_REFINEMENT_STORED_INACTIVE
                                                                                     : (uint32_t)state);
    le32(status + 8, state == FW_REFINEMENT_INSTALLED ? FW_REFINEMENT_PAYLOAD_BYTES : 0U);
    le32(status + 12, crc);
    request(FW_CMD_PRESS_REFINEMENT_STATUS, NULL, 0, 0);
    expect(FW_RESP_ACK, status, sizeof(status));
}

/* Arbitrary test data. Expected CRCs were calculated independently using zlib.crc32. */
static const uint32_t crcs[] = {UINT32_C(0x71cf3c96), UINT32_C(0x0cea0f80)};
static void make_record(uint8_t *out, unsigned seed) {
    memcpy(out, "OFRM", 4);
    le32(out + 4, 1);
    le32(out + 8, FW_REFINEMENT_PAYLOAD_BYTES);
    le32(out + 12, crcs[seed]);
    for (size_t i = 0; i < FW_REFINEMENT_PAYLOAD_BYTES; i++)
        out[FW_REFINEMENT_HEADER_BYTES + i] = (uint8_t)(i * 37 + 11 + seed);
}

void test_fw_refinement(void) {
    static uint8_t first[FW_REFINEMENT_RECORD_BYTES], next[FW_REFINEMENT_RECORD_BYTES];
    fw_usb_hooks_t hooks = {.on_refinement_installed = activate, .refinement_active = refinement_active};
    fw_usb_rx_init(&usb, NULL, &hooks);
    assert(fw_lfs_host_format_and_mount() == 0);
    expect_status(FW_REFINEMENT_ABSENT, 0);
    request(FW_CMD_GET_PRESS_REFINEMENT, NULL, 0, 0);
    expect(FW_RESP_NAK, NULL, 0);

    make_record(first, 0);
    make_record(next, 1);
    request(FW_CMD_PUT_PRESS_REFINEMENT, first, sizeof(first), sizeof(first));
    expect(FW_RESP_ACK, NULL, 0);
    expect_activation(1, first);
    assert(fw_lfs_host_unmount_and_remount() == 0);
    expect_status(FW_REFINEMENT_INSTALLED, crcs[0]);
    request(FW_CMD_GET_PRESS_REFINEMENT, NULL, 0, 0);
    expect(FW_RESP_ACK, first, sizeof(first));

    /* Not even the prefix of an interrupted or malformed replacement may go live. */
    request(FW_CMD_PUT_PRESS_REFINEMENT, next, sizeof(next), FW_USB_PACKET_SIZE * 5);
    assert(bytes_sent == 0 && fw_usb_rx_busy(&usb));
    fw_usb_rx_reset(&usb);
    size_t damaged[] = {0, 4, 8, 12, sizeof(next) - 1};
    for (unsigned i = 0; i < sizeof(damaged) / sizeof(damaged[0]); i++) {
        next[damaged[i]] ^= 1;
        request(FW_CMD_PUT_PRESS_REFINEMENT, next, sizeof(next), sizeof(next));
        expect(FW_RESP_NAK, NULL, 0);
        next[damaged[i]] ^= 1;
    }
    request(FW_CMD_PUT_PRESS_REFINEMENT, next, sizeof(next) - 1, sizeof(next) - 1);
    expect(FW_RESP_NAK, NULL, 0);
    request(FW_CMD_PRESS_REFINEMENT_STATUS, next, 1, 1);
    expect(FW_RESP_NAK, NULL, 0);
    request(FW_CMD_GET_PRESS_REFINEMENT, next, 1, 1);
    expect(FW_RESP_NAK, NULL, 0);

    expect_status(FW_REFINEMENT_INSTALLED, crcs[0]);
    expect_activation(1, first);

    /* Interrupt the flash program operations, including those after staging was written.
     * A fresh mount still reads the last ACKed model; a repeated install can recover. */
    unsigned cutoff = 0;
    for (; cutoff < 40; cutoff++) {
        fw_lfs_host_fail_program_after((int)cutoff);
        request(FW_CMD_PUT_PRESS_REFINEMENT, next, sizeof(next), sizeof(next));
        fw_lfs_host_fail_program_after(-1);
        if (response[0] == FW_RESP_ACK)
            break;
        expect(FW_RESP_NAK, NULL, 0);
        expect_activation(1, first);
        assert(fw_lfs_host_unmount_and_remount() == 0);
        request(FW_CMD_GET_PRESS_REFINEMENT, NULL, 0, 0);
        expect(FW_RESP_ACK, first, sizeof(first));
    }
    assert(cutoff > 0 && cutoff < 40);
    expect(FW_RESP_ACK, NULL, 0);
    expect_activation(2, next);
    assert(fw_lfs_host_unmount_and_remount() == 0);
    expect_status(FW_REFINEMENT_INSTALLED, crcs[1]);
    request(FW_CMD_GET_PRESS_REFINEMENT, NULL, 0, 0);
    expect(FW_RESP_ACK, next, sizeof(next));

    /* Model storage is optional: a corrupt file is distinguishable from a missing one,
     * never served as a usable record, and can be replaced without formatting flash. */
    assert(fw_lfs_save_u32("press_refinement_v1", 0U) == 0);
    expect_status(FW_REFINEMENT_INVALID, 0);
    request(FW_CMD_GET_PRESS_REFINEMENT, NULL, 0, 0);
    expect(FW_RESP_NAK, NULL, 0);
    request(FW_CMD_PUT_PRESS_REFINEMENT, first, sizeof(first), sizeof(first));
    expect(FW_RESP_ACK, NULL, 0);
    expect_status(FW_REFINEMENT_INSTALLED, crcs[0]);
}
