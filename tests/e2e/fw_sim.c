#define _POSIX_C_SOURCE 200809L

#include "fw_calibration.h"
#include "fw_hdlc.h"
#include "fw_protocol.h"
#include "fw_scanner.h"
#include "fw_usb_cmd.h"
#include "lfs_store.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static fw_runtime_t g_rt;
static fw_usb_rx_t g_usb_rx;
static fw_usb_tx_t g_usb_tx;
static fw_hdlc_decoder_t g_uart_dec;
static int g_uart_rx_fd = -1;
static int g_uart_tx_fd = -1;
static volatile sig_atomic_t g_run = 1;
static uint32_t g_sim_ms = 1000U;
static uint16_t g_raw_adc[FW_NUM_SENSORS];

static void on_sig(int sig) {
    (void)sig;
    g_run = 0;
}

static int on_save_keymap(const uint8_t *blob, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_save_keymap(blob, len);
}

static int on_save_ps(const uint8_t *xfer, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_save_profile_settings(xfer, len);
}

static int on_save_lang(uint32_t code, void *ctx) {
    (void)ctx;
    return fw_lfs_save_u32("language_code", code);
}

static int on_save_plat(uint32_t code, void *ctx) {
    (void)ctx;
    return fw_lfs_save_u32("platform_type_code", code);
}

static void on_refinement_installed(const uint8_t data[FW_REFINEMENT_PAYLOAD_BYTES], void *ctx) {
    (void)ctx;
    (void)fw_runtime_install_refinement(&g_rt, data, FW_REFINEMENT_PAYLOAD_BYTES);
}

static bool refinement_is_active(void *ctx) {
    (void)ctx;
    return fw_runtime_refinement_active(&g_rt);
}

static void on_uart_ack(const uint8_t *payload, size_t len, void *ctx) {
    (void)ctx;
    fw_runtime_handle_uart_response(&g_rt, payload, len);
}

/* Reads exactly len bytes; 0 on success, -1 on EOF or error. */
static int read_exact(int fd, void *buf, size_t len) {
    uint8_t *p = (uint8_t *)buf;
    size_t got = 0;
    while (got < len) {
        ssize_t r = read(fd, p + got, len - got);
        if (r > 0) got += (size_t)r;
        else if (r < 0 && errno == EINTR) continue;
        else return -1;
    }
    return 0;
}

/* Writes all len bytes; 0 on success, -1 on error. */
static int write_all(int fd, const void *buf, size_t len) {
    const uint8_t *p = (const uint8_t *)buf;
    size_t off = 0;
    while (off < len) {
        ssize_t w = write(fd, p + off, len - off);
        if (w > 0) off += (size_t)w;
        else if (w < 0 && errno == EINTR) continue;
        else return -1;
    }
    return 0;
}

static void send_uart_update(void) {
    if (g_uart_tx_fd < 0) return;
    uint8_t msg[FW_UART_MSG_UPDATE_LEN];
    fw_runtime_build_msg_update(&g_rt, g_sim_ms, msg);
    uint8_t wire[FW_HDLC_MAX_LEN * 2U + 16U];
    int n = fw_hdlc_encode(msg, sizeof(msg), wire, sizeof(wire));
    if (n > 0) {
        (void)write_all(g_uart_tx_fd, wire, (size_t)n);
    }
}

typedef struct {
    uint8_t buf[65536];
    size_t len;
} usb_reply_acc_t;

static uint32_t acc_usb_avail(void *ctx) {
    usb_reply_acc_t *acc = (usb_reply_acc_t *)ctx;
    return (acc->len < sizeof(acc->buf)) ? (uint32_t)(sizeof(acc->buf) - acc->len) : 0U;
}

static uint32_t acc_usb_write(const uint8_t *pkt64, uint32_t len, void *ctx) {
    usb_reply_acc_t *acc = (usb_reply_acc_t *)ctx;
    if (acc->len + len > sizeof(acc->buf)) return 0U;
    memcpy(acc->buf + acc->len, pkt64, len);
    acc->len += len;
    return len;
}

static void sim_scan(fw_hid_report_t *hid) {
    fw_runtime_set_time(&g_rt, g_sim_ms);
    fw_runtime_process_scan(&g_rt, g_raw_adc, hid);
    g_sim_ms++;
    (void)fw_runtime_finish_scan(&g_rt, g_sim_ms);
}

static int listen_unix(const char *path) {
    unlink(path);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    struct sockaddr_un sa;
    memset(&sa, 0, sizeof(sa));
    sa.sun_family = AF_UNIX;
    snprintf(sa.sun_path, sizeof(sa.sun_path), "%s", path);
    if (bind(fd, (struct sockaddr *)&sa, sizeof(sa)) != 0 || listen(fd, 8) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

/* Handles one length-prefixed packet sequence over AF_UNIX from host test USB adapter:
 * Request:  [uint32_le n_packets] followed by n_packets * [uint32_le pkt_len][bytes]
 * Response: [uint32_le resp_total_bytes][bytes...] */
static void handle_usb_conn(int cfd) {
    uint32_t npkts = 0;
    if (read_exact(cfd, &npkts, sizeof(npkts)) != 0) {
        close(cfd);
        return;
    }
    usb_reply_acc_t acc;
    memset(&acc, 0, sizeof(acc));
    fw_usb_tx_reset(&g_usb_tx);
    for (uint32_t i = 0; i < npkts; i++) {
        uint32_t plen = 0;
        if (read_exact(cfd, &plen, sizeof(plen)) != 0 || plen > 256U) {
            close(cfd);
            return;
        }
        uint8_t pkt[256];
        if (read_exact(cfd, pkt, plen) != 0) {
            close(cfd);
            return;
        }
        fw_usb_rx_feed_async(&g_usb_rx, &g_usb_tx, pkt, plen);
        while (fw_usb_tx_busy(&g_usb_tx)) {
            uint32_t sent = fw_usb_tx_step(&g_usb_tx, g_sim_ms, 4U, acc_usb_avail, acc_usb_write, NULL, &acc);
            if (sent == 0U) break;
            fw_hid_report_t hid;
            sim_scan(&hid);
        }
    }
    uint32_t rlen = (uint32_t)acc.len;
    if (write_all(cfd, &rlen, sizeof(rlen)) == 0 && rlen > 0) {
        (void)write_all(cfd, acc.buf, rlen);
    }
    close(cfd);
    g_sim_ms += 5U;
    send_uart_update();
}

/* Control socket accepts commands:
 *   "SCAN <s_pos>=<val>,<s_neg>=<val>" -> holds ADC position for 10 simulated scan ticks, emits UART MSG_UPDATE, prints HID JSON
 *   "RESET_ADC" -> resets all 160 ADC channels to 1800
 *   "REBOOT" -> simulates power-cycle reboot restoring persisted LittleFS into fresh g_rt */
static void handle_ctrl_conn(int cfd) {
    char cmd[1024];
    ssize_t n = read(cfd, cmd, sizeof(cmd) - 1);
    if (n <= 0) {
        close(cfd);
        return;
    }
    cmd[n] = '\0';
    if (strncmp(cmd, "RESET_ADC", 9) == 0) {
        for (size_t i = 0; i < FW_NUM_SENSORS; i++) g_raw_adc[i] = 1800U;
    } else if (strncmp(cmd, "SCAN ", 5) == 0) {
        char *p = cmd + 5;
        while (*p) {
            char *eq = strchr(p, '=');
            if (!eq) break;
            int idx = atoi(p);
            int val = atoi(eq + 1);
            if (idx >= 0 && idx < (int)FW_NUM_SENSORS) {
                g_raw_adc[idx] = (uint16_t)val;
            }
            char *comma = strchr(eq + 1, ',');
            if (!comma) break;
            p = comma + 1;
        }
    } else if (strncmp(cmd, "REBOOT", 6) == 0) {
        fw_runtime_init(&g_rt);
        fw_runtime_set_keyframe(&g_rt, 1, 1, 1);
        if (fw_lfs_host_unmount_and_remount() == 0) {
            fw_lfs_load_boot_state(&g_rt);
#if FW_ENABLE_PRESS_REFINEMENT
            if (fw_lfs_read_refinement(g_usb_rx.rx_buf) == FW_REFINEMENT_INSTALLED)
                on_refinement_installed(g_usb_rx.rx_buf + FW_REFINEMENT_HEADER_BYTES, NULL);
#endif
        }
        for (size_t i = 0; i < FW_NUM_SENSORS; i++) g_raw_adc[i] = 1800U;
    }

    fw_hid_report_t hid;
    for (unsigned tick = 0; tick < 10; tick++)
        sim_scan(&hid);
    send_uart_update();

    char nkro_hex[65];
    for (size_t i = 0; i < 32; i++) {
        snprintf(nkro_hex + i * 2, 3, "%02x", hid.nkro_bitmap[i]);
    }
    nkro_hex[64] = '\0';

    char resp[640];
    snprintf(resp, sizeof(resp),
             "{\"modifiers\":%u,\"keycodes\":[%u,%u,%u,%u,%u,%u],\"nkro_bitmap\":\"%s\",\"consumer\":%u,"
             "\"active_layer_bitfield\":%u,\"active_actuation_mode\":%u,\"active_profile\":%u,"
             "\"language_code\":%u,\"platform_code\":%u,\"keymap_len\":%zu}\n",
             hid.modifiers, hid.keycodes[0], hid.keycodes[1], hid.keycodes[2],
             hid.keycodes[3], hid.keycodes[4], hid.keycodes[5], nkro_hex, hid.consumer_usage,
             g_rt.effective_layer_bitfield ? g_rt.effective_layer_bitfield : g_rt.active_layer_bitfield,
             g_rt.active_actuation_mode, g_rt.active_profile,
             g_rt.language_code, g_rt.platform_code, g_rt.keymap_len);
    (void)write_all(cfd, resp, strlen(resp));
    close(cfd);
}

int main(void) {
    const char *ufds = getenv("OPENFLUX_SIM_UART_FDS");
    const char *usb_sock_path = getenv("OPENFLUX_SIM_USB_SOCK");
    const char *ctrl_sock_path = getenv("OPENFLUX_SIM_CTRL_SOCK");
    if (!ufds || !usb_sock_path || !ctrl_sock_path) {
        fprintf(stderr, "Missing OPENFLUX_SIM_UART_FDS / OPENFLUX_SIM_USB_SOCK / OPENFLUX_SIM_CTRL_SOCK\n");
        return 2;
    }
    if (sscanf(ufds, "%d,%d", &g_uart_rx_fd, &g_uart_tx_fd) != 2) {
        return 2;
    }

    signal(SIGPIPE, SIG_IGN);
    signal(SIGTERM, on_sig);
    signal(SIGINT, on_sig);

    fw_runtime_init(&g_rt);
    fw_runtime_set_keyframe(&g_rt, 1, 1, 1);
    if (fw_lfs_host_format_and_mount() != 0) {
        fprintf(stderr, "LittleFS init failed\n");
        return 1;
    }
    for (size_t i = 0; i < FW_NUM_SENSORS; i++) g_raw_adc[i] = 1800U;

    fw_usb_hooks_t hooks = {
        .on_refinement_installed = on_refinement_installed,
        .refinement_active = refinement_is_active,
        .on_keymap_saved = on_save_keymap,
        .on_profile_settings_saved = on_save_ps,
        .on_language_saved = on_save_lang,
        .on_platform_saved = on_save_plat,
    };
    fw_usb_rx_init(&g_usb_rx, &g_rt, &hooks);
    fw_usb_tx_init(&g_usb_tx);
    fw_hdlc_decoder_init(&g_uart_dec);

    int usb_srv = listen_unix(usb_sock_path);
    int ctrl_srv = listen_unix(ctrl_sock_path);
    if (usb_srv < 0 || ctrl_srv < 0) return 1;

    send_uart_update();

    while (g_run) {
        struct pollfd pfds[3] = {
            {.fd = usb_srv, .events = POLLIN, .revents = 0},
            {.fd = ctrl_srv, .events = POLLIN, .revents = 0},
            {.fd = g_uart_rx_fd, .events = POLLIN, .revents = 0},
        };
        if (poll(pfds, 3, 250) < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (pfds[0].revents & POLLIN) {
            int cfd = accept(usb_srv, NULL, NULL);
            if (cfd >= 0) handle_usb_conn(cfd);
        }
        if (pfds[1].revents & POLLIN) {
            int cfd = accept(ctrl_srv, NULL, NULL);
            if (cfd >= 0) handle_ctrl_conn(cfd);
        }
        if (pfds[2].revents & POLLIN) {
            uint8_t buf[256];
            ssize_t r = read(g_uart_rx_fd, buf, sizeof(buf));
            if (r > 0) fw_hdlc_decoder_feed(&g_uart_dec, buf, (size_t)r, on_uart_ack, NULL);
        }
    }

    close(usb_srv);
    close(ctrl_srv);
    unlink(usb_sock_path);
    unlink(ctrl_sock_path);
    return 0;
}
