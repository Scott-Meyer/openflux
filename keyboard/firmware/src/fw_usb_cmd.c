#include "fw_usb_cmd.h"
#include "lfs_store.h"

#include <string.h>

static uint32_t rd_le32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void wr_le32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)(v & 0xFFU);
    p[1] = (uint8_t)((v >> 8) & 0xFFU);
    p[2] = (uint8_t)((v >> 16) & 0xFFU);
    p[3] = (uint8_t)((v >> 24) & 0xFFU);
}

static void build_resp_hdr(uint8_t hdr[FW_USB_PACKET_SIZE], uint8_t resp_type, uint32_t len) {
    memset(hdr, 0, FW_USB_PACKET_SIZE);
    uint32_t pkts = (len > 0U) ? ((len + FW_USB_PACKET_SIZE - 1U) / FW_USB_PACKET_SIZE) : 0U;
    hdr[0] = resp_type;
    wr_le32(hdr + 1, pkts);
    wr_le32(hdr + 5, len);
}

static void send_response(fw_usb_send_fn_t send_fn, void *ctx, fw_usb_rx_t *rx, uint8_t resp_type, const uint8_t *data,
                          uint32_t len) {
    if (!send_fn) {
        if (ctx) {
            uint8_t *backing = rx ? rx->rx_buf : NULL;
            uint32_t cap = rx ? (uint32_t)sizeof(rx->rx_buf) : 0U;
            (void)fw_usb_tx_queue((fw_usb_tx_t *)ctx, resp_type, data, len, backing, cap);
        }
        return;
    }
    uint8_t hdr[FW_USB_PACKET_SIZE];
    build_resp_hdr(hdr, resp_type, len);
    if (send_fn(hdr, sizeof(hdr), ctx) != 0)
        return;

    uint32_t off = 0;
    while (off < len && data != NULL) {
        uint8_t pkt[FW_USB_PACKET_SIZE];
        memset(pkt, 0, sizeof(pkt));
        uint32_t chunk = len - off;
        if (chunk > FW_USB_PACKET_SIZE)
            chunk = FW_USB_PACKET_SIZE;
        memcpy(pkt, data + off, chunk);
        if (send_fn(pkt, sizeof(pkt), ctx) != 0)
            return;
        off += chunk;
    }
    (void)send_fn(hdr, 0U, ctx);
}

static void dispatch_command(fw_usb_rx_t *rx, fw_usb_send_fn_t send_fn, void *send_ctx) {
    fw_runtime_t *rt = rx->rt;
    uint32_t plen = rx->received_bytes;
    const uint8_t *p = rx->rx_buf;

    switch (rx->cmd) {
        case FW_CMD_REQUEST_VERSION: {
            send_response(send_fn, send_ctx, rx, FW_RESP_ACK, (const uint8_t *)FW_VERSION_STR,
                          (uint32_t)(strlen(FW_VERSION_STR) + 1U));
            break;
        }
        case FW_CMD_SEND_KEYMAP: {
            if (fw_runtime_validate_keymap(p, plen) == 0 &&
                (!rx->hooks.on_keymap_saved || rx->hooks.on_keymap_saved(p, plen, rx->hooks.ctx) == 0) &&
                fw_runtime_load_keymap(rt, p, plen) == 0) {
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_GET_KEYMAP: {
            if (rt->keymap_len > 0U) {
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, rt->keymap_blob, (uint32_t)rt->keymap_len);
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_MCU_BOOTLOADER: {
            send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
            if (rx->hooks.on_mcu_bootloader)
                rx->hooks.on_mcu_bootloader(rx->hooks.ctx);
            break;
        }
        case FW_CMD_SOC_BOOTLOADER: {
            send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
            if (rx->hooks.on_soc_bootloader)
                rx->hooks.on_soc_bootloader(rx->hooks.ctx);
            break;
        }
        case FW_CMD_SET_LANGUAGE: {
            if (plen >= 4U) {
                uint32_t code = rd_le32(p);
                if (!rx->hooks.on_language_saved || rx->hooks.on_language_saved(code, rx->hooks.ctx) == 0) {
                    rt->language_code = code;
                    send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
                } else {
                    send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
                }
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_SET_PLATFORM: {
            if (plen >= 4U) {
                uint32_t code = rd_le32(p);
                if (!rx->hooks.on_platform_saved || rx->hooks.on_platform_saved(code, rx->hooks.ctx) == 0) {
                    rt->platform_code = code;
                    send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
                } else {
                    send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
                }
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_SET_ACTIVE_PROFILE: {
            if (plen >= 4U && fw_runtime_set_active_profile(rt, rd_le32(p)) == 0) {
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_PRESS_REFINEMENT_STATUS:
        case FW_CMD_GET_PRESS_REFINEMENT: {
            if (plen != 0) {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
                break;
            }
            /* No request payload: reuse the USB scratch buffer for bounded, validated readback. */
            fw_refinement_state_t state = fw_lfs_read_refinement(rx->rx_buf);
            if (rx->cmd == FW_CMD_PRESS_REFINEMENT_STATUS) {
                uint8_t status[16] = {0};
                wr_le32(status, FW_REFINEMENT_FORMAT);
                wr_le32(status + 4, (uint32_t)state);
                if (state == FW_REFINEMENT_INSTALLED) {
                    if (!FW_ENABLE_PRESS_REFINEMENT || !rx->hooks.refinement_active ||
                        !rx->hooks.refinement_active(rx->hooks.ctx))
                        wr_le32(status + 4, FW_REFINEMENT_STORED_INACTIVE);
                    memcpy(status + 8, rx->rx_buf + 8, 8); /* payload length and CRC */
                }
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, status, sizeof(status));
            } else if (state == FW_REFINEMENT_INSTALLED) {
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, rx->rx_buf, FW_REFINEMENT_RECORD_BYTES);
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_PUT_PRESS_REFINEMENT: {
            if (fw_lfs_install_refinement(p, plen) != 0) {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
                break;
            }
            if (FW_ENABLE_PRESS_REFINEMENT && rx->hooks.on_refinement_installed)
                rx->hooks.on_refinement_installed(p + FW_REFINEMENT_HEADER_BYTES, rx->hooks.ctx);
            send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
            break;
        }
        case FW_CMD_FILE_FROM_HOST: {
            if (fw_runtime_validate_profile_settings(p, plen) == 0 &&
                (!rx->hooks.on_profile_settings_saved ||
                 rx->hooks.on_profile_settings_saved(p, plen, rx->hooks.ctx) == 0) &&
                fw_runtime_load_profile_settings(rt, p, plen) == 0) {
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, NULL, 0U);
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_FILE_TO_HOST: {
            if (plen >= 4U && rd_le32(p) != 0U) {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
                break;
            }
            uint8_t out[16 + FW_MAX_PROFILES * 8U];
            size_t out_len = 0;
            if (fw_runtime_export_profile_settings(rt, out, sizeof(out), &out_len) == 0) {
                send_response(send_fn, send_ctx, rx, FW_RESP_ACK, out, (uint32_t)out_len);
            } else {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            }
            break;
        }
        case FW_CMD_GET_RUNTIME_STATE: {
            if (plen != 0U) {
                send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
                break;
            }
            uint8_t out[12];
            wr_le32(out + 0, rt->active_profile);
            wr_le32(out + 4, rt->active_actuation_mode);
            wr_le32(out + 8, rt->platform_code);
            send_response(send_fn, send_ctx, rx, FW_RESP_ACK, out, sizeof(out));
            break;
        }
        default:
            send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            break;
    }
}

void fw_usb_tx_init(fw_usb_tx_t *tx) {
    if (!tx)
        return;
    memset(tx, 0, sizeof(*tx));
}

void fw_usb_tx_reset(fw_usb_tx_t *tx) {
    if (!tx)
        return;
    tx->active = 0U;
    tx->hdr_sent = 0U;
    tx->progress_started = 0U;
    tx->payload = NULL;
    tx->payload_len = 0U;
    tx->payload_off = 0U;
    tx->last_progress_ms = 0U;
}

int fw_usb_tx_busy(const fw_usb_tx_t *tx) {
    return tx != NULL && tx->active != 0U;
}

int fw_usb_tx_queue(fw_usb_tx_t *tx, uint8_t resp_type, const uint8_t *data, uint32_t len, uint8_t *backing_buf,
                    uint32_t backing_cap) {
    if (!tx || (len > 0U && !data))
        return -1;
    if (len > 0U) {
        if (backing_buf != NULL) {
            if (len > backing_cap)
                return -1;
            if (data != backing_buf)
                memmove(backing_buf, data, len);
            tx->payload = backing_buf;
        } else {
            tx->payload = data;
        }
    } else {
        tx->payload = NULL;
    }
    build_resp_hdr(tx->hdr, resp_type, len);
    tx->payload_len = len;
    tx->payload_off = 0U;
    tx->hdr_sent = 0U;
    tx->progress_started = 0U;
    tx->last_progress_ms = 0U;
    tx->active = 1U;
    return 0;
}

uint32_t fw_usb_tx_step(fw_usb_tx_t *tx, uint32_t now_ms, uint32_t max_packets, fw_usb_tx_avail_fn_t avail_fn,
                        fw_usb_tx_write_fn_t write_fn, fw_usb_tx_flush_fn_t flush_fn, void *ctx) {
    if (!tx || !tx->active || !avail_fn || !write_fn)
        return 0U;
    if (!tx->progress_started) {
        tx->last_progress_ms = now_ms;
        tx->progress_started = 1U;
    }
    if (max_packets == 0U)
        max_packets = 1U;

    uint32_t sent = 0U;
    while (sent < max_packets && tx->active) {
        if (avail_fn(ctx) < FW_USB_PACKET_SIZE) {
            if (sent == 0U && (uint32_t)(now_ms - tx->last_progress_ms) >= FW_USB_TX_STALL_TIMEOUT_MS) {
                fw_usb_tx_reset(tx);
            }
            break;
        }
        uint8_t pkt[FW_USB_PACKET_SIZE];
        if (!tx->hdr_sent) {
            memcpy(pkt, tx->hdr, FW_USB_PACKET_SIZE);
        } else {
            memset(pkt, 0, sizeof(pkt));
            uint32_t rem = tx->payload_len - tx->payload_off;
            uint32_t chunk = (rem > FW_USB_PACKET_SIZE) ? FW_USB_PACKET_SIZE : rem;
            if (chunk > 0U && tx->payload != NULL) {
                memcpy(pkt, tx->payload + tx->payload_off, chunk);
            }
        }
        uint32_t written = write_fn(pkt, FW_USB_PACKET_SIZE, ctx);
        if (written != FW_USB_PACKET_SIZE) {
            break;
        }
        sent++;
        tx->last_progress_ms = now_ms;
        if (!tx->hdr_sent) {
            tx->hdr_sent = 1U;
        } else {
            uint32_t rem = tx->payload_len - tx->payload_off;
            uint32_t chunk = (rem > FW_USB_PACKET_SIZE) ? FW_USB_PACKET_SIZE : rem;
            tx->payload_off += chunk;
        }
        if (tx->hdr_sent && tx->payload_off >= tx->payload_len) {
            tx->active = 0U;
        }
    }
    if (sent > 0U && flush_fn) {
        flush_fn(ctx);
    }
    return sent;
}

void fw_usb_rx_feed_async(fw_usb_rx_t *rx, fw_usb_tx_t *tx, const uint8_t *data, size_t len) {
    fw_usb_rx_feed(rx, data, len, NULL, tx);
}

void fw_usb_rx_init(fw_usb_rx_t *rx, fw_runtime_t *rt, const fw_usb_hooks_t *hooks) {
    if (!rx)
        return;
    memset(rx, 0, sizeof(*rx));
    rx->rt = rt;
    if (hooks)
        rx->hooks = *hooks;
}

void fw_usb_rx_reset(fw_usb_rx_t *rx) {
    if (!rx)
        return;
    rx->state = FW_RX_IDLE;
    rx->expected_packets = 0;
    rx->expected_bytes = 0;
    rx->received_packets = 0;
    rx->received_bytes = 0;
}

int fw_usb_rx_busy(const fw_usb_rx_t *rx) {
    return rx != NULL && rx->state != FW_RX_IDLE;
}

void fw_usb_rx_feed(fw_usb_rx_t *rx, const uint8_t *data, size_t len, fw_usb_send_fn_t send_fn, void *send_ctx) {
    if (!rx || (!data && len > 0U))
        return;
    if (len == 0U)
        return;

    if (rx->state == FW_RX_IDLE) {
        if (len < 9U) {
            send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            return;
        }
        rx->cmd = data[0];
        rx->expected_packets = rd_le32(data + 1);
        rx->expected_bytes = rd_le32(data + 5);
        rx->received_packets = 0;
        rx->received_bytes = 0;
        /* A request carries exactly ceil(bytes / P) packets. Any other count can't be trusted, even
         * to skip the packets that follow, so it is refused at once. */
        uint64_t needed_packets = ((uint64_t)rx->expected_bytes + FW_USB_PACKET_SIZE - 1U) / FW_USB_PACKET_SIZE;
        if ((uint64_t)rx->expected_packets != needed_packets) {
            send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            return;
        }
        if (rx->expected_packets == 0U) {
            dispatch_command(rx, send_fn, send_ctx);
            return;
        }
        bool wrong_model_size =
            rx->cmd == FW_CMD_PUT_PRESS_REFINEMENT && rx->expected_bytes != FW_REFINEMENT_RECORD_BYTES;
        bool unexpected_payload = rx->cmd == FW_CMD_PRESS_REFINEMENT_STATUS || rx->cmd == FW_CMD_GET_PRESS_REFINEMENT;
        if (rx->expected_bytes > FW_MAX_KEYMAP_BYTES || wrong_model_size || unexpected_payload) {
            rx->state = FW_RX_DISCARDING;
            return;
        }
        rx->state = FW_RX_RECEIVING;
        return;
    }

    if (rx->state == FW_RX_DISCARDING) {
        rx->received_packets++;
        if (rx->received_packets >= rx->expected_packets) {
            fw_usb_rx_reset(rx);
            send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
        }
        return;
    }

    size_t remain = (rx->expected_bytes > rx->received_bytes) ? (rx->expected_bytes - rx->received_bytes) : 0U;
    size_t copy = (len < remain) ? len : remain;
    if (copy > 0U) {
        memcpy(rx->rx_buf + rx->received_bytes, data, copy);
        rx->received_bytes += (uint32_t)copy;
    }
    rx->received_packets++;
    if (rx->received_packets >= rx->expected_packets) {
        rx->state = FW_RX_IDLE;
        if (rx->received_bytes != rx->expected_bytes) {
            /* Short packets left the payload incomplete. */
            send_response(send_fn, send_ctx, rx, FW_RESP_NAK, NULL, 0U);
            return;
        }
        dispatch_command(rx, send_fn, send_ctx);
    }
}
