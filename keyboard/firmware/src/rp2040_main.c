#include "hardware/adc.h"
#include "hardware/gpio.h"
#include "hardware/uart.h"
#include "hardware/watchdog.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "fw_gamepad.h"
#include "fw_hdlc.h"
#include "fw_protocol.h"
#include "fw_scanner.h"
#include "fw_usb_cmd.h"
#include "lfs_store.h"
#include "qmk/fw_qmk.h"

#include <string.h>

static fw_runtime_t g_rt;
static fw_usb_rx_t g_usb_rx;
static fw_usb_tx_t g_usb_tx;
static fw_hdlc_decoder_t g_uart_dec;
static volatile uint8_t g_reboot_mcu = 0;
static volatile uint8_t g_reboot_soc = 0;
static volatile uint8_t g_usb_reenumerate = 0;
static volatile uint8_t g_usb_config_changed = 0;

/* Hall-effect Analog Gamepad state + App-Linking layer override. */
static fw_gamepad_config_t g_gamepad_cfg;
static bool g_gamepad_cfg_dirty = false;
static volatile uint8_t g_usb_gamepad_mounted = 0U;
static uint8_t g_layer_override = 0xFFU; /* 0..7 = active App-Linking layer, 0xFF = profile default */
static uint16_t g_gp_resolved_kc[FW_MAX_KEYS_PER_LAYOUT];
static bool g_gp_resolved_valid = false;
static uint8_t g_gp_last_wire[FW_GP_REPORT_WIRE_LEN];
static bool g_gp_has_sent = false;

bool fw_usb_gamepad_enabled(void) {
    return g_gamepad_cfg.mode != FW_GP_MODE_OFF;
}

/* Ring buffers decoupling TinyUSB callbacks from the QMK scan step (zero blocking!). */
static uint8_t g_vial_rx_ring[4][FW_QMK_VIAL_RAW_EPSIZE];
static uint8_t g_vial_rx_head = 0U;
static uint8_t g_vial_rx_tail = 0U;

static uint8_t g_vial_tx_ring[4][FW_QMK_VIAL_RAW_EPSIZE];
static uint8_t g_vial_tx_head = 0U;
static uint8_t g_vial_tx_tail = 0U;

typedef struct {
    uint16_t slot;
    uint8_t pressed;
    uint8_t mods;
    uint8_t layer;
} rp2040_flux_evt_t;

static rp2040_flux_evt_t g_flux_hid_ring[16];
static uint8_t g_flux_hid_head = 0U;
static uint8_t g_flux_hid_tail = 0U;

/* Debounced RAM staging for dynamic XZ vial.json uploaded over IF 1. */
static uint8_t g_vial_def_stage[FW_QMK_MAX_VIAL_DEF_BYTES];
static size_t g_vial_def_stage_len = 0U;
static bool g_vial_def_dirty = false;
static uint32_t g_vial_def_dirty_ms = 0U;
static bool g_language_dirty = false;
static uint32_t g_language_pending = 0U;

/* Non-blocking 2048-byte UART1 TX ring buffer so display telemetry never stalls ADC scans. */
#define RP2040_UART_TX_RING_CAP 2048U
static uint8_t g_uart_tx_ring[RP2040_UART_TX_RING_CAP];
static uint16_t g_uart_tx_head = 0U;
static uint16_t g_uart_tx_tail = 0U;

static void uart_tx_drain(void) {
    while (g_uart_tx_tail != g_uart_tx_head && uart_is_writable(uart1)) {
        uart_putc_raw(uart1, (char)g_uart_tx_ring[g_uart_tx_tail]);
        g_uart_tx_tail = (uint16_t)((g_uart_tx_tail + 1U) & (RP2040_UART_TX_RING_CAP - 1U));
    }
}

enum { BRIDGE_HEARTBEAT_MS = 1000 };
/* Responses acknowledge the message type, not a sequence number. Only a recent successful
 * response proves the bridge is answering. Zero is already expired at boot or after a reset. */
static uint64_t g_bridge_ack_expires_us;

/* TinyUSB calls these from tud_task() when the host configures the device or the device is
 * reset or unplugged. */
void tud_mount_cb(void) {
    g_usb_config_changed = 1U;
    g_usb_gamepad_mounted = fw_usb_gamepad_enabled() ? 1U : 0U;
}
void tud_umount_cb(void) {
    g_usb_config_changed = 1U;
    g_usb_gamepad_mounted = 0U;
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
    /* Defer flash write to quiet-keys window so SET_LANGUAGE never stalls typing. */
    g_language_pending = code;
    g_language_dirty = true;
    return 0;
}
static int on_platform_saved(uint32_t code, void *ctx) {
    (void)ctx;
    g_rt.platform_code = code;
    g_rt.platform_dirty = 1U;
    return 0;
}

static void on_uart_frame(const uint8_t *payload, size_t len, void *ctx) {
    (void)ctx;
    if (len != 4 || (payload[0] != FW_UART_MSG_UPDATE && payload[0] != FW_UART_MSG_QC_SENSORS &&
                     payload[0] != FW_UART_MSG_ANALOG) || payload[1] > 1 ||
        payload[2] > (FW_UART_FLAG_QC_MODE | FW_UART_FLAG_ANALOG_MODE) || payload[3] > 1)
        return;
    if (payload[1] == 0) {
        g_bridge_ack_expires_us = time_us_64() + 3ULL * BRIDGE_HEARTBEAT_MS * 1000;
    }
    fw_runtime_handle_uart_response(&g_rt, payload, len);
}

static void uart_send_hdlc(const uint8_t *payload, size_t len) {
    /* Non-blocking HDLC enqueue into UART1 TX ring (atomic frame: drop only if ring is full). */
    static uint8_t wire[FW_HDLC_MAX_LEN * 2U + 16U];
    uart_tx_drain();
    int n = fw_hdlc_encode(payload, len, wire, sizeof(wire));
    if (n <= 0)
        return;
    uint16_t used = (uint16_t)((g_uart_tx_head - g_uart_tx_tail) & (RP2040_UART_TX_RING_CAP - 1U));
    uint16_t avail = (uint16_t)((RP2040_UART_TX_RING_CAP - 1U) - used);
    if ((uint16_t)n > avail)
        return;
    for (int i = 0; i < n; i++) {
        g_uart_tx_ring[g_uart_tx_head] = wire[i];
        g_uart_tx_head = (uint16_t)((g_uart_tx_head + 1U) & (RP2040_UART_TX_RING_CAP - 1U));
    }
    uart_tx_drain();
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
    /* Report ID 3 on IF 4 (legacy 4-byte OpenFlux mask, compatible with existing openfluxd). */
    return tud_hid_n_report(1, 3, le, sizeof(le)) ? 0 : -1;
}

void fw_on_hid_led_report(uint8_t led_mask) {
    g_rt.host_state_status = led_mask;
}

void fw_on_vial_raw_hid_rx(const uint8_t *buffer, uint16_t bufsize) {
    if (!buffer || bufsize == 0U)
        return;
    uint8_t next = (uint8_t)((g_vial_rx_head + 1U) & 3U);
    if (next == g_vial_rx_tail)
        return; /* drop if overloaded */
    memset(g_vial_rx_ring[g_vial_rx_head], 0, FW_QMK_VIAL_RAW_EPSIZE);
    size_t n = (bufsize < FW_QMK_VIAL_RAW_EPSIZE) ? (size_t)bufsize : FW_QMK_VIAL_RAW_EPSIZE;
    memcpy(g_vial_rx_ring[g_vial_rx_head], buffer, n);
    g_vial_rx_head = next;
}

static int qmk_cb_eeprom_load(uint8_t layout_id, uint8_t *buf, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_load_qmk_eeprom(layout_id, buf, len);
}

static int qmk_cb_eeprom_save(uint8_t layout_id, const uint8_t *buf, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_save_qmk_eeprom(layout_id, buf, len);
}

static void qmk_cb_raw_hid_send(const uint8_t data[FW_QMK_VIAL_RAW_EPSIZE], void *ctx) {
    (void)ctx;
    /* Non-blocking enqueue so Vial traffic NEVER stalls the 1000 Hz Hall-effect scan loop. */
    if (g_vial_tx_tail == g_vial_tx_head && tud_hid_n_ready(2)) {
        if (tud_hid_n_report(2, 0, data, FW_QMK_VIAL_RAW_EPSIZE))
            return;
    }
    uint8_t next = (uint8_t)((g_vial_tx_head + 1U) & 3U);
    if (next == g_vial_tx_tail) {
        g_vial_tx_tail = (uint8_t)((g_vial_tx_tail + 1U) & 3U);
    }
    memcpy(g_vial_tx_ring[g_vial_tx_head], data, FW_QMK_VIAL_RAW_EPSIZE);
    g_vial_tx_head = next;
}

static void qmk_cb_on_flux_action(uint16_t slot, bool pressed, uint8_t effective_mods, uint32_t active_layers,
                                  void *ctx) {
    (void)ctx;
    fw_runtime_handle_of_action(&g_rt, slot, pressed);

    uint8_t top_layer = 0U;
    for (int l = 7; l >= 0; l--) {
        if ((active_layers >> (uint32_t)l) & 1U) {
            top_layer = (uint8_t)l;
            break;
        }
    }

    /* Emit unacknowledged FW_UART_MSG_FLUX_ACTION = 5 to the Linux display SoC. */
    uint8_t frame[12];
    uint32_t seq = ++g_rt.uart_tx_seq;
    frame[0] = FW_UART_MSG_FLUX_ACTION;
    frame[1] = (uint8_t)(seq & 0xFFU);
    frame[2] = (uint8_t)((seq >> 8) & 0xFFU);
    frame[3] = (uint8_t)((seq >> 16) & 0xFFU);
    frame[4] = (uint8_t)((seq >> 24) & 0xFFU);
    frame[5] = (uint8_t)(slot & 0xFFU);
    frame[6] = (uint8_t)(slot >> 8);
    frame[7] = pressed ? 1U : 0U;
    frame[8] = effective_mods;
    frame[9] = top_layer;
    frame[10] = (uint8_t)(g_rt.active_profile & 0xFFU);
    frame[11] = 0U;
    uart_send_hdlc(frame, sizeof(frame));

    /* Queue generic action report (Report ID 4) for USB Vendor HID IF 4. */
    uint8_t next = (uint8_t)((g_flux_hid_head + 1U) & 15U);
    if (next != g_flux_hid_tail) {
        g_flux_hid_ring[g_flux_hid_head].slot = slot;
        g_flux_hid_ring[g_flux_hid_head].pressed = pressed ? 1U : 0U;
        g_flux_hid_ring[g_flux_hid_head].mods = effective_mods;
        g_flux_hid_ring[g_flux_hid_head].layer = top_layer;
        g_flux_hid_head = next;
    }
}

static bool qmk_cb_hid_drain_one(const fw_hid_report_t *rep, void *ctx) {
    (void)ctx;
    if (!rep)
        return false;
    /* Called only when a giant >120-char macro fills the 256-entry FIFO: briefly yield to USB
     * so one packet can transmit instead of dropping macro characters. */
    uint32_t start = to_ms_since_boot(get_absolute_time());
    while (!tud_hid_n_ready(0) && (uint32_t)(to_ms_since_boot(get_absolute_time()) - start) < 5U) {
        tud_task();
    }
    if (!tud_hid_n_ready(0))
        return false;
    g_last_hid = *rep;
    return rp2040_send_kbd(rep->modifiers, rep->keycodes, rep->nkro_bitmap, NULL) == 0;
}

static uint8_t qmk_cb_get_host_leds(void *ctx) {
    (void)ctx;
    return g_rt.host_state_status;
}

static int on_vial_def_uploaded(const uint8_t *xz_data, size_t len, void *ctx) {
    (void)ctx;
    if (!xz_data || len == 0U || len > sizeof(g_vial_def_stage))
        return -1;
    if (fw_qmk_set_vial_def(xz_data, len, NULL) != 0)
        return -1;
    memcpy(g_vial_def_stage, xz_data, len);
    g_vial_def_stage_len = len;
    g_vial_def_dirty = true;
    g_vial_def_dirty_ms = to_ms_since_boot(get_absolute_time());
    return 0;
}

static int on_gamepad_config(const uint8_t *data, size_t len, void *ctx) {
    (void)ctx;
    fw_gamepad_config_t next;
    if (fw_gamepad_config_decode(data, len, &next) != 0)
        return -1;
    bool was_enabled = (g_gamepad_cfg.mode != FW_GP_MODE_OFF);
    bool now_enabled = (next.mode != FW_GP_MODE_OFF);
    g_gamepad_cfg = next;
    g_gamepad_cfg_dirty = true;
    g_gp_has_sent = false;
    if (was_enabled != now_enabled) {
        /* Never touch HID instance 3 until the host mounts the new USB configuration after reboot. */
        g_usb_gamepad_mounted = 0U;
        g_usb_reenumerate = 1U;
    }
    return 0;
}

static int on_layer_override(uint8_t layer, void *ctx) {
    (void)ctx;
    if (layer == 0xFFU) {
        g_layer_override = 0xFFU;
        (void)fw_runtime_set_active_profile(&g_rt, g_rt.active_profile);
        g_gp_resolved_valid = false;
        return 0;
    }
    if (layer >= 8U)
        return -1;
    g_layer_override = layer;
    fw_qmk_set_default_layer(layer);
    g_gp_resolved_valid = false;
    return 0;
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
    fw_gamepad_config_default(&g_gamepad_cfg);
    if (fw_lfs_mount() == 0) {
        fw_lfs_load_boot_state(&g_rt);
        uint8_t gp_buf[FW_GP_CONFIG_WIRE_LEN];
        if (fw_lfs_load_gamepad_cfg(gp_buf, sizeof(gp_buf)) == 0) {
            (void)fw_gamepad_config_decode(gp_buf, sizeof(gp_buf), &g_gamepad_cfg);
        }
#if FW_ENABLE_PRESS_REFINEMENT
        /* The command receiver is idle before USB initialization. Use its transfer buffer instead of the stack. */
        if (fw_lfs_read_refinement(g_usb_rx.rx_buf) == FW_REFINEMENT_INSTALLED)
            on_refinement_installed(g_usb_rx.rx_buf + FW_REFINEMENT_HEADER_BYTES, NULL);
#endif
    }

    static const fw_qmk_platform_t qmk_plat = {
        .eeprom_load = qmk_cb_eeprom_load,
        .eeprom_save = qmk_cb_eeprom_save,
        .raw_hid_send = qmk_cb_raw_hid_send,
        .hid_drain_one = qmk_cb_hid_drain_one,
        .on_flux_action = qmk_cb_on_flux_action,
        .get_host_leds = qmk_cb_get_host_leds,
        .ctx = NULL,
    };
    fw_qmk_init(&qmk_plat, 1U, to_ms_since_boot(get_absolute_time()));
    /* One-time upgrade migration: seed QMK EEPROM from existing LittleFS keymap if no Vial EEPROM exists yet. */
    (void)fw_qmk_seed_from_legacy_keymap(&g_rt, 1U);
    {
        size_t def_len = 0U;
        if (fw_lfs_load_vial_def(g_vial_def_stage, sizeof(g_vial_def_stage), &def_len) == 0) {
            (void)fw_qmk_set_vial_def(g_vial_def_stage, def_len, NULL);
        }
    }
    g_rt.qmk_enabled = 1U;
    (void)fw_runtime_set_active_profile(&g_rt, g_rt.active_profile);

    static const fw_usb_hooks_t hooks = {
        .on_mcu_bootloader = on_mcu_bootloader,
        .on_soc_bootloader = on_soc_bootloader,
        .on_keymap_saved = on_keymap_saved,
        .on_profile_settings_saved = on_profile_saved,
        .on_refinement_installed = on_refinement_installed,
        .refinement_active = refinement_is_active,
        .on_vial_def_uploaded = on_vial_def_uploaded,
        .on_gamepad_config = on_gamepad_config,
        .on_layer_override = on_layer_override,
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
    static fw_hid_report_t qmk_cur_hid;
    static bool qmk_has_pending_hid = false;
    static uint8_t last_bitmap[32];
    /* These messages are built and consumed serially, never accessed by an interrupt. */
    static union {
        uint8_t update[FW_UART_MSG_UPDATE_LEN];
        uint8_t qc[FW_UART_MSG_QC_LEN];
        uint8_t analog[FW_UART_MSG_ANALOG_MAX_LEN];
        uint8_t resolved[320];
    } io;
    uint32_t last_analog_tx_ms = 0U;
    uint32_t last_resolved_tx_ms = 0U;
    bool resolved_dirty = true;
    uint8_t last_leds = 0U;
    uint32_t last_profile = g_rt.active_profile;
    uint32_t last_act_mode = g_rt.active_actuation_mode;
    uint32_t last_platform = g_rt.platform_code;
    uint32_t last_eff_layer = g_rt.effective_layer_bitfield;
    uint16_t last_of_event_seq = g_rt.of_event_seq;
    fw_qmk_state_t last_qmk_state;
    fw_qmk_get_state(&last_qmk_state);
    uint8_t last_qmk_layout = 1U;
    uint32_t last_full_scan_ms = to_ms_since_boot(get_absolute_time());
    uint32_t last_uart_ms = last_full_scan_ms - BRIDGE_HEARTBEAT_MS;
    uint32_t last_key_activity_ms = last_full_scan_ms;

    for (;;) {
        tud_task();
        uart_tx_drain();

        if (g_usb_config_changed) {
            /* A new USB configuration knows nothing of earlier reports or of a half-received
             * command: resend the current key state and wait for a fresh command header. */
            g_usb_config_changed = 0U;
            memset(&hid_sched, 0, sizeof(hid_sched));
            fw_qmk_clear_hid_queue();
            qmk_has_pending_hid = false;
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
        if (g_usb_reenumerate && !fw_usb_tx_busy(&g_usb_tx)) {
            g_usb_reenumerate = 0U;
            g_usb_gamepad_mounted = 0U;
            /* Flush gamepad config + any pending QMK EEPROM before clean hardware USB reset. */
            if (g_gamepad_cfg_dirty) {
                uint8_t gp_buf[FW_GP_CONFIG_WIRE_LEN];
                fw_gamepad_config_encode(&g_gamepad_cfg, gp_buf);
                (void)fw_lfs_save_gamepad_cfg(gp_buf, sizeof(gp_buf));
                g_gamepad_cfg_dirty = false;
            }
            (void)fw_qmk_poll_eeprom_flush(to_ms_since_boot(get_absolute_time()), 0U, true);
            /* Pump tud_task briefly so the 0x74 ACK ZLP reaches the host before resetting USB. */
            for (int i = 0; i < 40; i++) {
                tud_task();
                sleep_ms(1);
            }
            tud_disconnect();
            sleep_ms(80);
            watchdog_reboot(0, 0, 10);
            for (;;) {
                tight_loop_contents();
            }
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
        /* Drain queued Vial Raw HID TX responses without ever blocking the scan loop. */
        if (g_vial_tx_tail != g_vial_tx_head && tud_hid_n_ready(2)) {
            if (tud_hid_n_report(2, 0, g_vial_tx_ring[g_vial_tx_tail], FW_QMK_VIAL_RAW_EPSIZE)) {
                g_vial_tx_tail = (uint8_t)((g_vial_tx_tail + 1U) & 3U);
            }
        }
        /* Drain pending Vial Raw HID packets before scanning so edits take effect cleanly. */
        while (g_vial_rx_tail != g_vial_rx_head) {
            fw_qmk_raw_hid_rx(g_vial_rx_ring[g_vial_rx_tail], scan_ms);
            g_vial_rx_tail = (uint8_t)((g_vial_rx_tail + 1U) & 3U);
        }

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
        if (g_rt.keyframe_attached && g_rt.keyframe_layout >= 1U && g_rt.keyframe_layout <= 3U &&
            g_rt.keyframe_layout != last_qmk_layout) {
            fw_qmk_set_layout(g_rt.keyframe_layout, scan_ms);
            (void)fw_qmk_seed_from_legacy_keymap(&g_rt, g_rt.keyframe_layout);
            /* Preserve active App-Linking layer override or profile default layer across hot-swaps. */
            if (g_layer_override < 8U) {
                fw_qmk_set_default_layer(g_layer_override);
            } else {
                (void)fw_runtime_set_active_profile(&g_rt, g_rt.active_profile);
            }
            g_gp_resolved_valid = false;
            last_qmk_layout = g_rt.keyframe_layout;
            resolved_dirty = true;
        }
        fw_hid_report_t hid;
        bool kbd_ep_ready = tud_hid_n_ready(0);
        fw_runtime_set_time(&g_rt, scan_ms);
        fw_runtime_set_hid_ready(&g_rt, kbd_ep_ready ? 1U : 0U);
        fw_runtime_process_scan(&g_rt, raw_adc, &hid);
        uint32_t now_ms = to_ms_since_boot(get_absolute_time());
        bool save_rest = fw_runtime_finish_scan(&g_rt, now_ms);
        if (save_rest)
            (void)fw_lfs_save_rest_ref(&g_rt);
        /* Only flush dirty LittleFS files when NO keys are held AND typing has been quiet >=500ms,
         * so flash erase/program IRQ-disabled windows never interrupt active typing. */
        bool any_key_down = false;
        for (size_t i = 0; i < sizeof(g_rt.key_bitmap); i++) {
            if (g_rt.key_bitmap[i] != 0U) {
                any_key_down = true;
                break;
            }
        }
        /* Wake a suspended host computer on any physical key press (matches USB config 0xA0). */
        if (any_key_down && tud_suspended()) {
            (void)tud_remote_wakeup();
        }
        if (any_key_down || memcmp(last_bitmap, g_rt.key_bitmap, sizeof(last_bitmap)) != 0) {
            last_key_activity_ms = now_ms;
        }
        bool keys_quiet = !any_key_down && (uint32_t)(now_ms - last_key_activity_ms) >= 500U;
        if (keys_quiet) {
            if (g_rt.platform_dirty) {
                (void)fw_lfs_save_u32("platform_type_code", g_rt.platform_code);
                g_rt.platform_dirty = 0U;
            }
            if (g_language_dirty) {
                (void)fw_lfs_save_u32("language_code", g_language_pending);
                g_language_dirty = false;
            }
            if (g_gamepad_cfg_dirty) {
                uint8_t gp_buf[FW_GP_CONFIG_WIRE_LEN];
                fw_gamepad_config_encode(&g_gamepad_cfg, gp_buf);
                (void)fw_lfs_save_gamepad_cfg(gp_buf, sizeof(gp_buf));
                g_gamepad_cfg_dirty = false;
            }
        }
        (void)fw_qmk_poll_eeprom_flush(now_ms, 500U, keys_quiet);
        if (g_vial_def_dirty && keys_quiet && (uint32_t)(now_ms - g_vial_def_dirty_ms) >= 500U) {
            (void)fw_lfs_save_vial_def(g_vial_def_stage, g_vial_def_stage_len);
            g_vial_def_dirty = false;
        }
        /* Drain QMK's HID report FIFO one report per USB poll opportunity so back-to-back
         * press+release transitions emitted inside one scan (mod-tap/layer-tap taps, fast rolls,
         * combos, macros) are NEVER overwritten before reaching the host OS! */
        if (g_rt.qmk_enabled) {
            if (!qmk_has_pending_hid && fw_qmk_pop_hid_report(&qmk_cur_hid)) {
                qmk_has_pending_hid = true;
            }
            const fw_hid_report_t *tx_hid = qmk_has_pending_hid ? &qmk_cur_hid : &hid;
            g_last_hid = *tx_hid;
            fw_hid_scheduler_step(&hid_sched, tx_hid, kbd_ep_ready, tud_hid_n_ready(1), rp2040_send_kbd,
                                  rp2040_send_consumer, rp2040_send_of_func, NULL);
            if (qmk_has_pending_hid && hid_sched.initialized &&
                hid_sched.last_modifiers == qmk_cur_hid.modifiers &&
                hid_sched.last_consumer_usage == qmk_cur_hid.consumer_usage &&
                memcmp(hid_sched.last_nkro_bitmap, qmk_cur_hid.nkro_bitmap, sizeof(qmk_cur_hid.nkro_bitmap)) == 0) {
                qmk_has_pending_hid = false;
            }
        } else {
            g_last_hid = hid;
            fw_hid_scheduler_step(&hid_sched, &hid, kbd_ep_ready, tud_hid_n_ready(1), rp2040_send_kbd,
                                  rp2040_send_consumer, rp2040_send_of_func, NULL);
        }
        if (g_flux_hid_tail != g_flux_hid_head && tud_hid_n_ready(1)) {
            rp2040_flux_evt_t ev = g_flux_hid_ring[g_flux_hid_tail];
            uint8_t pkt[8] = {
                (uint8_t)(ev.slot & 0xFFU), (uint8_t)(ev.slot >> 8), ev.pressed, ev.mods, ev.layer, 0U, 0U, 0U,
            };
            if (tud_hid_n_report(1, 4, pkt, sizeof(pkt))) {
                g_flux_hid_tail = (uint8_t)((g_flux_hid_tail + 1U) & 15U);
            }
        }

        fw_qmk_state_t cur_qmk_state;
        fw_qmk_get_state(&cur_qmk_state);
        int qmk_changed = (memcmp(&cur_qmk_state, &last_qmk_state, sizeof(cur_qmk_state)) != 0);
        int qmk_faces_changed = (cur_qmk_state.layer_state != last_qmk_state.layer_state ||
                                 cur_qmk_state.default_layer_state != last_qmk_state.default_layer_state ||
                                 cur_qmk_state.keymap_revision != last_qmk_state.keymap_revision);
        if (!qmk_faces_changed &&
            (cur_qmk_state.effective_mods != last_qmk_state.effective_mods ||
             cur_qmk_state.oneshot_mods != last_qmk_state.oneshot_mods) &&
            fw_qmk_has_enabled_key_overrides()) {
            qmk_faces_changed = 1;
        }
        if (qmk_faces_changed) {
            resolved_dirty = true;
            g_gp_resolved_valid = false;
        }
        /* 1000 Hz Hall-effect Analog Gamepad evaluation & USB HID transmission (IF 6, instance 3).
         * Strictly gated on g_usb_gamepad_mounted && !g_usb_reenumerate so HID instance 3 is never
         * accessed when the active USB configuration only opened HID instances 0..2. */
        if (g_gamepad_cfg.mode != FW_GP_MODE_OFF && g_usb_gamepad_mounted && !g_usb_reenumerate) {
            uint8_t glid = (g_rt.keyframe_layout >= 1U && g_rt.keyframe_layout <= 3U) ? g_rt.keyframe_layout : 1U;
            uint16_t gn_keys = 0U;
            for (uint8_t r = 0U; r < FW_MATRIX_ROWS; r++) {
                if (FW_ROW_LEN[glid][r] > 1U)
                    gn_keys = (uint16_t)(gn_keys + (FW_ROW_LEN[glid][r] - 1U));
            }
            if (!g_gp_resolved_valid) {
                for (uint16_t k = 0U; k < gn_keys && k < FW_MAX_KEYS_PER_LAYOUT; k++) {
                    g_gp_resolved_kc[k] = fw_qmk_resolve_key(k, NULL);
                }
                g_gp_resolved_valid = true;
            }
            fw_gamepad_report_t gp_rep;
            uint8_t gp_wire[FW_GP_REPORT_WIRE_LEN];
            fw_gamepad_evaluate(&g_gamepad_cfg, g_rt.flux, g_rt.key_pressed, g_gp_resolved_kc, gn_keys, &gp_rep);
            fw_gamepad_report_pack(&gp_rep, gp_wire);
            if ((!g_gp_has_sent || memcmp(gp_wire, g_gp_last_wire, sizeof(gp_wire)) != 0) && tud_hid_n_ready(3)) {
                if (tud_hid_n_report(3, 0, gp_wire, sizeof(gp_wire))) {
                    memcpy(g_gp_last_wire, gp_wire, sizeof(gp_wire));
                    g_gp_has_sent = true;
                }
            }
        }
        int keys_changed = (memcmp(last_bitmap, g_rt.key_bitmap, sizeof(last_bitmap)) != 0);
        int leds_changed = (g_rt.host_state_status != last_leds);
        int state_changed = (g_rt.active_profile != last_profile || g_rt.active_actuation_mode != last_act_mode ||
                             g_rt.platform_code != last_platform || g_rt.effective_layer_bitfield != last_eff_layer ||
                             g_rt.of_event_seq != last_of_event_seq || qmk_changed);
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
            last_qmk_state = cur_qmk_state;
            last_uart_ms = now_ms;
        }
        if (resolved_dirty && (now_ms - last_resolved_tx_ms) >= 20U) {
            size_t rlen = fw_runtime_build_msg_resolved_keys(&g_rt, io.resolved, sizeof(io.resolved));
            if (rlen > 0U) {
                uart_send_hdlc(io.resolved, rlen);
            }
            resolved_dirty = false;
            last_resolved_tx_ms = now_ms;
        }
        if (g_rt.analog_mode && (now_ms - last_analog_tx_ms) >= 8U) {
            size_t alen = fw_runtime_build_msg_analog(&g_rt, now_ms, io.analog);
            if (alen > 0U) {
                uart_send_hdlc(io.analog, alen);
            }
            last_analog_tx_ms = now_ms;
        }
        if (g_rt.qc_mode && full_scan) {
            fw_runtime_build_msg_qc(&g_rt, io.qc);
            uart_send_hdlc(io.qc, sizeof(io.qc));
        }
    }
}
