#include "hardware/adc.h"
#include "hardware/gpio.h"
#include "hardware/uart.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "fw_hdlc.h"
#include "fw_protocol.h"
#include "fw_scanner.h"
#include "fw_usb_cmd.h"
#include "lfs_store.h"

#include <string.h>

static fw_runtime_t g_rt;
static fw_usb_rx_t g_usb_rx;
static fw_usb_tx_t g_usb_tx;
static fw_hdlc_decoder_t g_uart_dec;
static volatile uint8_t g_reboot_mcu = 0;
static volatile uint8_t g_reboot_soc = 0;
static volatile uint8_t g_usb_config_changed = 0;

enum { BRIDGE_HEARTBEAT_MS = 1000 };
/* Responses acknowledge the message type, not a sequence number. Only a recent successful
 * response proves the bridge is answering. Zero is already expired at boot or after a reset. */
static uint64_t g_bridge_ack_expires_us;

/* TinyUSB calls these from tud_task() when the host configures the device or the device is
 * reset or unplugged. */
void tud_mount_cb(void) {
    g_usb_config_changed = 1U;
}
void tud_umount_cb(void) {
    g_usb_config_changed = 1U;
}

static void init_hardware(void) {
    /* SoC power/aux/reset GPIOs */
    gpio_init(FW_SOC_AUX_GPIO);
    gpio_set_dir(FW_SOC_AUX_GPIO, GPIO_OUT);
    gpio_put(FW_SOC_AUX_GPIO, 1);

    gpio_init(FW_SOC_PWR_GPIO);
    gpio_set_dir(FW_SOC_PWR_GPIO, GPIO_OUT);
    gpio_put(FW_SOC_PWR_GPIO, 1);

    gpio_init(FW_SOC_RST_GPIO);
    gpio_set_dir(FW_SOC_RST_GPIO, GPIO_OUT);
    gpio_put(FW_SOC_RST_GPIO, 0);

    /* Column MUX GPIOs 10..17 */
    gpio_init_mask(FW_MUX_COL_MASK);
    gpio_set_dir_out_masked(FW_MUX_COL_MASK);

    /* Break before make: start deselected, then park on the first bank between sweeps. */
    for (size_t b = 0; b < FW_NUM_BANKS; b++) {
        gpio_init(FW_BANK_PINS[b]);
        gpio_put(FW_BANK_PINS[b], 1);
        gpio_set_dir(FW_BANK_PINS[b], GPIO_OUT);
    }
    gpio_put(FW_BANK_PINS[0], 0);

    /* ADC3 on GPIO 29 */
    adc_init();
    adc_gpio_init(FW_ADC_GPIO);
    adc_select_input(FW_ADC_CHANNEL);

    /* UART1 on GPIO 8 (TX) / GPIO 9 (RX) @ 921600 8N1 */
    uart_init(uart1, FW_UART_BAUD);
    uart_set_format(uart1, 8, 1, UART_PARITY_NONE);
    gpio_set_function(FW_UART_TX_GPIO, GPIO_FUNC_UART);
    gpio_set_function(FW_UART_RX_GPIO, GPIO_FUNC_UART);
}

/* count is either the 112 keyframe channels or all 160 channels. Bank zero is active
 * on entry and exit. At a bank boundary, disable the old bank before enabling the new one. */
static void __no_inline_not_in_flash_func(scan_sensors)(uint16_t raw[FW_NUM_SENSORS], unsigned count) {
    unsigned bank = 0;
    gpio_put_masked(FW_MUX_COL_MASK, fw_mux_col_gpio_value(0));
    busy_wait_us_32(2);
    hw_set_bits(&adc_hw->cs, ADC_CS_START_ONCE_BITS);
    for (unsigned slot = 0; slot < count; slot++) {
        unsigned next = slot + 1;
        if (next < count) {
            unsigned next_bank = next / FW_COLS_PER_BANK;
            if (next_bank != bank) {
                gpio_put(FW_BANK_PINS[bank], 1);
                bank = next_bank;
                gpio_put(FW_BANK_PINS[bank], 0);
            }
            /* The ADC is converting this slot; let the next analog input settle meanwhile. */
            gpio_put_masked(FW_MUX_COL_MASK, fw_mux_col_gpio_value(next % FW_COLS_PER_BANK));
        }
        while (!(adc_hw->cs & ADC_CS_READY_BITS))
            tight_loop_contents();
        uint16_t reading = (uint16_t)adc_hw->result;
        if (next < count)
            hw_set_bits(&adc_hw->cs, ADC_CS_START_ONCE_BITS);
        raw[slot] = reading;
    }
    if (bank != 0) {
        gpio_put(FW_BANK_PINS[bank], 1);
        gpio_put(FW_BANK_PINS[0], 0);
    }
}

typedef struct {
    uint16_t mean[FW_NUM_SENSORS];
    uint8_t unstable[FW_REST_CHANNELS];
} rest_measurement_t;

/* Keep measurement scratch off the RP2040 stack. `raw` retains the last actual ADC frame,
 * not the averaged reference, so sensing and QC can use it immediately after warm-up. */
static void measure_rest(uint16_t raw[FW_NUM_SENSORS], rest_measurement_t *rest) {
    static uint16_t low[FW_REST_CHANNELS], high[FW_REST_CHANNELS];
    enum { sweeps = 8 };
    _Static_assert(sweeps * 4095U <= UINT16_MAX, "ADC sums must fit");
    memset(rest->mean, 0, sizeof(rest->mean));
    for (unsigned pass = 0; pass < sweeps; pass++) {
        scan_sensors(raw, FW_NUM_SENSORS);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
            rest->mean[i] += raw[i];
            if (i < FW_REST_CHANNELS) {
                if (!pass || raw[i] < low[i])
                    low[i] = raw[i];
                if (!pass || raw[i] > high[i])
                    high[i] = raw[i];
            }
        }
        sleep_ms(2);
    }
    for (size_t i = 0; i < FW_NUM_SENSORS; i++)
        rest->mean[i] /= sweeps;
    for (size_t i = 0; i < FW_REST_CHANNELS; i++)
        rest->unstable[i] = high[i] - low[i] > 40;
}

static uint32_t usb_vendor_tx_avail(void *ctx) {
    (void)ctx;
    return tud_vendor_write_available();
}

static uint32_t usb_vendor_tx_write(const uint8_t *pkt64, uint32_t len, void *ctx) {
    (void)ctx;
    return tud_vendor_write(pkt64, len);
}

static void usb_vendor_tx_flush(void *ctx) {
    (void)ctx;
    (void)tud_vendor_write_flush();
}

/* Single bounded IF1 RX/TX service called after tud_task() before sensor acquisition.
 * Never spins on endpoint backpressure or calls tud_task() recursively. */
static void rp2040_usb_vendor_service(void) {
    static uint8_t rx_pkt[FW_USB_PACKET_SIZE];
    static uint32_t last_usb_rx_ms;
    enum { MAX_PACKETS_PER_ITER = 4U };
    uint32_t now_ms = to_ms_since_boot(get_absolute_time());

    (void)fw_usb_tx_step(&g_usb_tx, now_ms, MAX_PACKETS_PER_ITER, usb_vendor_tx_avail, usb_vendor_tx_write,
                         usb_vendor_tx_flush, NULL);
    for (uint32_t i = 0U; i < MAX_PACKETS_PER_ITER && !fw_usb_tx_busy(&g_usb_tx) && tud_vendor_available() > 0U; i++) {
        uint32_t n = tud_vendor_read(rx_pkt, sizeof(rx_pkt));
        if (n > 0U) {
            fw_usb_rx_feed_async(&g_usb_rx, &g_usb_tx, rx_pkt, n);
            last_usb_rx_ms = now_ms;
        }
    }
    if (fw_usb_tx_busy(&g_usb_tx)) {
        (void)fw_usb_tx_step(&g_usb_tx, now_ms, MAX_PACKETS_PER_ITER, usb_vendor_tx_avail, usb_vendor_tx_write,
                             usb_vendor_tx_flush, NULL);
    }
    if (fw_usb_rx_busy(&g_usb_rx) && (now_ms - last_usb_rx_ms) >= FW_USB_RX_IDLE_TIMEOUT_MS) {
        fw_usb_rx_reset(&g_usb_rx);
    }
}

static void on_mcu_bootloader(void *ctx) {
    (void)ctx;
    g_reboot_mcu = 1U;
}
static void on_soc_bootloader(void *ctx) {
    (void)ctx;
    g_reboot_soc = 1U;
}
static int on_keymap_saved(const uint8_t *blob, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_save_keymap(blob, len);
}
static int on_profile_saved(const uint8_t *xfer, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_save_profile_settings(xfer, len);
}
static int on_language_saved(uint32_t code, void *ctx) {
    (void)ctx;
    return fw_lfs_save_u32("language_code", code);
}
static int on_platform_saved(uint32_t code, void *ctx) {
    (void)ctx;
    return fw_lfs_save_u32("platform_type_code", code);
}

static void on_uart_frame(const uint8_t *payload, size_t len, void *ctx) {
    (void)ctx;
    if (len != 4 || (payload[0] != FW_UART_MSG_UPDATE && payload[0] != FW_UART_MSG_QC_SENSORS) || payload[1] > 1 ||
        payload[2] > 1 || payload[3] > 1)
        return;
    if (payload[1] == 0) {
        g_bridge_ack_expires_us = time_us_64() + 3ULL * BRIDGE_HEARTBEAT_MS * 1000;
    }
    fw_runtime_handle_uart_response(&g_rt, payload, len);
}

static void uart_send_hdlc(const uint8_t *payload, size_t len) {
    /* Only the foreground loop transmits; uart_write_blocking consumes the bytes before return. */
    static uint8_t wire[FW_HDLC_MAX_LEN * 2U + 16U];
    int n = fw_hdlc_encode(payload, len, wire, sizeof(wire));
    if (n > 0) {
        uart_write_blocking(uart1, wire, (size_t)n);
    }
}

static fw_hid_report_t g_last_hid;

static int rp2040_send_kbd(uint8_t modifiers, const uint8_t keycodes[6], const uint8_t nkro_bitmap[32], void *ctx) {
    (void)ctx;
    if (tud_hid_n_get_protocol(0) == HID_PROTOCOL_BOOT) {
        return tud_hid_n_keyboard_report(0, 0, modifiers, keycodes) ? 0 : -1;
    }
    uint8_t nkro_report[33];
    nkro_report[0] = 0x00U;
    memcpy(nkro_report + 1, nkro_bitmap, 32U);
    return tud_hid_n_report(0, 1, nkro_report, sizeof(nkro_report)) ? 0 : -1;
}

uint16_t fw_on_hid_get_report(uint8_t instance, uint8_t report_id, uint8_t *buffer, uint16_t reqlen) {
    if (!buffer)
        return 0;
    if (instance == 1U) {
        if (reqlen < 4U)
            return 0;
        uint32_t m = g_last_hid.of_func_mask;
        buffer[0] = (uint8_t)(m & 0xFFU);
        buffer[1] = (uint8_t)((m >> 8) & 0xFFU);
        buffer[2] = (uint8_t)((m >> 16) & 0xFFU);
        buffer[3] = (uint8_t)((m >> 24) & 0xFFU);
        return 4U;
    }
    if (instance != 0U)
        return 0;
    if (tud_hid_n_get_protocol(0) == HID_PROTOCOL_BOOT || report_id == 0U) {
        if (reqlen < 8U)
            return 0;
        buffer[0] = g_last_hid.modifiers;
        buffer[1] = 0x00U;
        memcpy(buffer + 2, g_last_hid.keycodes, 6U);
        return 8U;
    }
    if (report_id == 1U) {
        if (reqlen < 33U)
            return 0;
        buffer[0] = 0x00U;
        memcpy(buffer + 1, g_last_hid.nkro_bitmap, 32U);
        return 33U;
    }
    return 0;
}

void fw_on_hid_protocol_changed(uint8_t protocol) {
    (void)protocol;
    g_usb_config_changed = 1U;
}

static int rp2040_send_consumer(uint16_t usage, void *ctx) {
    (void)ctx;
    return tud_hid_n_report(0, 2, &usage, sizeof(usage)) ? 0 : -1;
}

static int rp2040_send_of_func(uint32_t func_mask, void *ctx) {
    (void)ctx;
    uint8_t le[4] = {
        (uint8_t)(func_mask & 0xFFU),
        (uint8_t)((func_mask >> 8) & 0xFFU),
        (uint8_t)((func_mask >> 16) & 0xFFU),
        (uint8_t)((func_mask >> 24) & 0xFFU),
    };
    return tud_hid_n_report(1, 0, le, sizeof(le)) ? 0 : -1;
}

void fw_on_hid_led_report(uint8_t led_mask) {
    g_rt.host_state_status = led_mask;
}

static void on_refinement_installed(const uint8_t data[FW_REFINEMENT_PAYLOAD_BYTES], void *ctx) {
    (void)ctx;
    (void)fw_runtime_install_refinement(&g_rt, data, FW_REFINEMENT_PAYLOAD_BYTES);
}
static bool refinement_is_active(void *ctx) {
    (void)ctx;
    return fw_runtime_refinement_active(&g_rt);
}

int main(void) {
    init_hardware();
    fw_runtime_init(&g_rt);
    if (fw_lfs_mount() == 0) {
        fw_lfs_load_boot_state(&g_rt);
#if FW_ENABLE_PRESS_REFINEMENT
        /* The command receiver is idle before USB initialization. Use its transfer buffer instead of the stack. */
        if (fw_lfs_read_refinement(g_usb_rx.rx_buf) == FW_REFINEMENT_INSTALLED)
            on_refinement_installed(g_usb_rx.rx_buf + FW_REFINEMENT_HEADER_BYTES, NULL);
#endif
    }

    static const fw_usb_hooks_t hooks = {
        .on_mcu_bootloader = on_mcu_bootloader,
        .on_soc_bootloader = on_soc_bootloader,
        .on_keymap_saved = on_keymap_saved,
        .on_profile_settings_saved = on_profile_saved,
        .on_refinement_installed = on_refinement_installed,
        .refinement_active = refinement_is_active,
        .on_language_saved = on_language_saved,
        .on_platform_saved = on_platform_saved,
        .ctx = NULL,
    };
    fw_usb_rx_init(&g_usb_rx, &g_rt, &hooks);
    fw_usb_tx_init(&g_usb_tx);
    fw_hdlc_decoder_init(&g_uart_dec);
    tusb_init();

    static uint16_t raw_adc[FW_NUM_SENSORS];
    static rest_measurement_t rest;
    /* With no identified frame yet, these are only initial sensor offsets. The first
     * identified layout and feel get their own measurement and saved reference below. */
    measure_rest(raw_adc, &rest);
    memcpy(g_rt.baseline_adc, rest.mean, sizeof rest.mean);

    static fw_hid_scheduler_t hid_sched;
    static uint8_t last_bitmap[32];
    /* These messages are built and consumed serially, never accessed by an interrupt. */
    static union {
        uint8_t update[FW_UART_MSG_UPDATE_LEN];
        uint8_t qc[FW_UART_MSG_QC_LEN];
    } io;
    uint8_t last_leds = 0U;
    uint32_t last_profile = g_rt.active_profile;
    uint32_t last_act_mode = g_rt.active_actuation_mode;
    uint32_t last_platform = g_rt.platform_code;
    uint32_t last_eff_layer = g_rt.effective_layer_bitfield;
    uint16_t last_of_event_seq = g_rt.of_event_seq;
    uint32_t last_full_scan_ms = to_ms_since_boot(get_absolute_time());
    uint32_t last_uart_ms = last_full_scan_ms - BRIDGE_HEARTBEAT_MS;

    for (;;) {
        tud_task();

        if (g_usb_config_changed) {
            /* A new USB configuration knows nothing of earlier reports or of a half-received
             * command: resend the current key state and wait for a fresh command header. */
            g_usb_config_changed = 0U;
            memset(&hid_sched, 0, sizeof(hid_sched));
            fw_usb_rx_reset(&g_usb_rx);
            fw_usb_tx_reset(&g_usb_tx);
        }

        rp2040_usb_vendor_service();

        while (uart_is_readable(uart1)) {
            uint8_t b = (uint8_t)uart_getc(uart1);
            fw_hdlc_decoder_feed(&g_uart_dec, &b, 1U, on_uart_frame, NULL);
        }

        if (g_reboot_mcu && !fw_usb_tx_busy(&g_usb_tx)) {
            sleep_ms(50);
            reset_usb_boot(0, 0);
        }
        if (g_reboot_soc && !fw_usb_tx_busy(&g_usb_tx)) {
            g_reboot_soc = 0U;
            gpio_init(FW_SOC_RST_GPIO);
            gpio_set_dir(FW_SOC_RST_GPIO, GPIO_OUT);
            gpio_put(FW_SOC_RST_GPIO, 1);
            gpio_put(FW_SOC_PWR_GPIO, 1);
            sleep_ms(200);
            while (uart_is_readable(uart1))
                (void)uart_getc(uart1);
            fw_hdlc_decoder_init(&g_uart_dec);
            g_bridge_ack_expires_us = 0;
            gpio_put(FW_SOC_RST_GPIO, 0);
        }

        uint32_t scan_ms = to_ms_since_boot(get_absolute_time());
        bool full_scan = (scan_ms - last_full_scan_ms) >= 16U;
        scan_sensors(raw_adc, full_scan ? FW_NUM_SENSORS : FW_REST_CHANNELS);
        if (full_scan)
            last_full_scan_ms = scan_ms;
        bool recalibrate = fw_runtime_poll_frame(&g_rt, raw_adc, scan_ms);
        if (recalibrate) {
            measure_rest(raw_adc, &rest);
            full_scan = true;
            last_full_scan_ms = to_ms_since_boot(get_absolute_time());
            (void)fw_lfs_load_rest_ref(&g_rt);
            uint32_t calibrated_ms = to_ms_since_boot(get_absolute_time());
            bool bridge_connected = time_us_64() < g_bridge_ack_expires_us;
            fw_runtime_calibrate(&g_rt, rest.mean, rest.unstable, calibrated_ms, bridge_connected);
        }
        fw_hid_report_t hid;
        fw_runtime_set_time(&g_rt, scan_ms);
        fw_runtime_process_scan(&g_rt, raw_adc, &hid);
        g_last_hid = hid;
        uint32_t now_ms = to_ms_since_boot(get_absolute_time());
        bool save_rest = fw_runtime_finish_scan(&g_rt, now_ms);
        if (save_rest)
            (void)fw_lfs_save_rest_ref(&g_rt);
        if (g_rt.platform_dirty) {
            (void)fw_lfs_save_u32("platform_type_code", g_rt.platform_code);
            g_rt.platform_dirty = 0U;
        }
        fw_hid_scheduler_step(&hid_sched, &hid, tud_hid_n_ready(0), tud_hid_n_ready(1), rp2040_send_kbd,
                              rp2040_send_consumer, rp2040_send_of_func, NULL);

        int keys_changed = (memcmp(last_bitmap, g_rt.key_bitmap, sizeof(last_bitmap)) != 0);
        int leds_changed = (g_rt.host_state_status != last_leds);
        int state_changed = (g_rt.active_profile != last_profile || g_rt.active_actuation_mode != last_act_mode ||
                             g_rt.platform_code != last_platform || g_rt.effective_layer_bitfield != last_eff_layer ||
                             g_rt.of_event_seq != last_of_event_seq);
        if (keys_changed || leds_changed || state_changed || (now_ms - last_uart_ms) >= BRIDGE_HEARTBEAT_MS) {
            fw_runtime_build_msg_update(&g_rt, now_ms, io.update);
            uart_send_hdlc(io.update, sizeof(io.update));
            memcpy(last_bitmap, g_rt.key_bitmap, sizeof(last_bitmap));
            last_of_event_seq = g_rt.of_event_seq;
            last_leds = g_rt.host_state_status;
            last_profile = g_rt.active_profile;
            last_act_mode = g_rt.active_actuation_mode;
            last_platform = g_rt.platform_code;
            last_eff_layer = g_rt.effective_layer_bitfield;
            last_uart_ms = now_ms;
        }
        if (g_rt.qc_mode && full_scan) {
            fw_runtime_build_msg_qc(&g_rt, io.qc);
            uart_send_hdlc(io.qc, sizeof(io.qc));
        }
    }
}
