#ifndef FW_SCANNER_H
#define FW_SCANNER_H

#include "fw_calibration.h"
#include "fw_protocol.h"
#include "fw_classic_keys.h"
#include "fw_classic_neighbors.h"
#include "fw_rapid.h"
#include "fw_lifecycle.h"
#include "fw_refinement.h"
#if FW_ENABLE_PRESS_REFINEMENT
#include "fw_depth.h"
#include "fw_refine.h"
#endif

#include <stddef.h>
#include <stdint.h>

#define FW_MAX_KEYMAP_BYTES    16384U
#define FW_MAX_PROFILES        64U
#define FW_MAX_KEYS_PER_LAYOUT 96U
#define FW_MAX_LAYERS          8U
#define FW_MAX_ACT_MODES       8U
#define FW_MATRIX_ROWS         6U
#define FW_MATRIX_MAX_COLS     17U

extern const uint8_t FW_BANK_PINS[FW_NUM_BANKS];
extern const uint8_t FW_ROW_LEN[4][FW_MATRIX_ROWS];
extern const uint8_t FW_SENSOR_MAP[4][FW_MATRIX_ROWS][FW_MATRIX_MAX_COLS];

/* Computes GPIO 10..17 state for differential column mux channel (0..15). */
uint32_t fw_mux_col_gpio_value(uint8_t col);

typedef struct {
    uint8_t valid;
    uint16_t num_act_modes;
    uint16_t num_keys;
    uint16_t num_layers;
    uint16_t num_macros;
    uint32_t macro_pool_words;
    uint32_t keycodes_word_off;
    uint32_t actuations_word_off;
    const uint32_t *section_words; /* points inside keymap_blob */
} fw_layout_view_t;

typedef struct {
    uint32_t actuation_mode_index;
    uint32_t default_layer_bitfield;
} fw_profile_setting_t;

typedef struct {
    uint8_t modifiers;
    uint8_t keycodes[6];
    uint8_t nkro_bitmap[32];
    uint16_t consumer_usage;
    uint32_t of_func_mask;
} fw_hid_report_t;

typedef struct {
    uint8_t initialized;
    uint8_t of_initialized;
    uint8_t last_modifiers;
    uint8_t last_keycodes[6];
    uint8_t last_nkro_bitmap[32];
    uint16_t last_consumer_usage;
    uint32_t last_of_func_mask;
    uint32_t of_queue[16];
    uint8_t of_q_head;
    uint8_t of_q_tail;
} fw_hid_scheduler_t;

typedef int (*fw_hid_send_kbd_fn_t)(uint8_t modifiers, const uint8_t keycodes[6], const uint8_t nkro_bitmap[32],
                                    void *ctx);
typedef int (*fw_hid_send_consumer_fn_t)(uint16_t usage, void *ctx);
typedef int (*fw_hid_send_of_fn_t)(uint32_t func_mask, void *ctx);

/* Sends at most one keyboard/consumer report per ready opportunity on EP 0x83, and drains the
 * independent vendor-HID queue on EP 0x85 whenever of_ep_ready is true. */
void fw_hid_scheduler_step(fw_hid_scheduler_t *sched, const fw_hid_report_t *desired, int ep_ready, int of_ep_ready,
                           fw_hid_send_kbd_fn_t send_kbd, fw_hid_send_consumer_fn_t send_consumer,
                           fw_hid_send_of_fn_t send_of_func, void *ctx);

typedef struct fw_runtime {
    fw_lifecycle_t lifecycle;
    fw_classic_settings_t classic_settings[FW_MATRIX_ROWS];
    fw_classic_keys_t classic_keys[FW_MATRIX_ROWS];
    fw_classic_neighbors_t classic_neighbors[FW_MATRIX_ROWS];
    enum { FW_CLASSIC_ATTACH, FW_CLASSIC_RELOAD, FW_CLASSIC_REPLACED, FW_CLASSIC_READY } classic_setup;
#if FW_ENABLE_PRESS_REFINEMENT
    bool refinement_active;
    fw_refine_model_t refinement;
    fw_depth_row_t depth_rows[FW_MATRIX_ROWS];
    fw_refine_row_t refine_rows[FW_MATRIX_ROWS];
#endif
    fw_calibration_t cal;
    uint16_t baseline_adc[FW_NUM_SENSORS];
    uint16_t raw_adc[FW_NUM_SENSORS];
    int32_t sensor_scaled[FW_NUM_SENSORS];
    uint16_t flux[FW_MAX_KEYS_PER_LAYOUT];
    fw_key_motion_t motion[FW_MAX_KEYS_PER_LAYOUT];
    fw_rapid_state_t rapid[FW_MAX_KEYS_PER_LAYOUT];
    uint8_t key_pressed[FW_MAX_KEYS_PER_LAYOUT];
    uint8_t fixed_transition_scans[FW_MAX_KEYS_PER_LAYOUT];
    uint8_t neighbor_vetoes[FW_MAX_KEYS_PER_LAYOUT];
    uint16_t last_scan_mode;
    uint8_t latched_valid[FW_MAX_KEYS_PER_LAYOUT];
    uint32_t latched_action[FW_MAX_KEYS_PER_LAYOUT];
    uint8_t key_bitmap[32];

    uint8_t keymap_blob[FW_MAX_KEYMAP_BYTES];
    size_t keymap_len;
    fw_layout_view_t layouts[4]; /* 1=ANSI, 2=ISO, 3=JIS */

    fw_profile_setting_t profiles[FW_MAX_PROFILES];
    uint32_t num_profiles;

    uint8_t keyframe_layout;   /* 1=ANSI, 2=ISO, 3=JIS */
    uint8_t keyframe_attached; /* 0/1 */
    uint8_t switch_feel;
    uint32_t active_layer_bitfield;    /* configured/default/switched base layer mask */
    uint32_t toggled_layer_bitfield;   /* layers latched on by toggleLayer (kind 0x2, type 2/4) */
    uint32_t effective_layer_bitfield; /* active_layer_bitfield | toggled_layer_bitfield | held Fn */
    uint32_t active_actuation_mode;
    uint32_t language_code;
    uint32_t platform_code;
    uint32_t active_profile;

    uint8_t macro_active;
    uint8_t macro_layout;
    uint8_t macro_clock_updated;
    uint16_t macro_id;
    uint32_t macro_step_idx;
    uint32_t macro_next_ms;
    uint32_t macro_clock_ms;
    uint8_t macro_nkro_bitmap[32];

    uint8_t qc_mode;
    uint8_t stress_test_enabled;
    uint8_t host_state_status; /* HID LED bitmask: 0x01 Num, 0x02 Caps, 0x04 Scroll */
    uint8_t platform_dirty;
    uint16_t of_event_seq;
    uint8_t of_event_ring[16];
    uint8_t of_event_profiles[16];
    uint8_t rest_ref_valid;
    int16_t rest_ref_adc[112];
    uint32_t uart_tx_seq;

#ifdef HOST_TEST
    /* Optional override matrix for unit tests; NULL uses FW_ROW_LEN/FW_SENSOR_MAP[lid]. */
    const uint8_t *test_row_len;
    const uint8_t (*test_sensor_map)[FW_MATRIX_MAX_COLS];
#endif
} fw_runtime_t;

void fw_runtime_init(fw_runtime_t *rt);

/* Updates keyframe attachment/layout (1=ANSI, 2=ISO, 3=JIS) and resets pressed/extrema state on change. */
void fw_runtime_set_keyframe(fw_runtime_t *rt, uint8_t attached, uint8_t layout, uint8_t feel);
void fw_runtime_set_time(fw_runtime_t *rt, uint32_t now_ms);

/* Invalidate signal history after calibration without releasing held actions. */
void fw_runtime_reset_tracking(fw_runtime_t *rt);
/* A validated payload can be activated only in an explicitly enabled build. */
int fw_runtime_install_refinement(fw_runtime_t *rt, const uint8_t *payload, size_t len);
bool fw_runtime_refinement_active(const fw_runtime_t *rt);

/* Validates a KeymapList binary blob without modifying runtime. Returns 0 or -1. */
int fw_runtime_validate_keymap(const uint8_t *blob, size_t len);

/* Validates and loads a KeymapList binary blob into runtime. Returns 0 or -1. */
int fw_runtime_load_keymap(fw_runtime_t *rt, const uint8_t *blob, size_t len);

/* Validates PROFILE_SETTINGS_V1 transfer payload without modifying runtime. Returns 0 or -1. */
int fw_runtime_validate_profile_settings(const uint8_t *xfer, size_t len);

/* Validates and applies PROFILE_SETTINGS_V1 transfer payload (16B FileHeader + 16B Header + entries). */
int fw_runtime_load_profile_settings(fw_runtime_t *rt, const uint8_t *xfer, size_t len);

/* Serializes current PROFILE_SETTINGS_V1 data bytes (16B Header + entries) for FILE_TO_HOST. */
int fw_runtime_export_profile_settings(const fw_runtime_t *rt, uint8_t *out, size_t cap, size_t *out_len);

/* Applies active profile index (updating active_actuation_mode and active_layer_bitfield). */
int fw_runtime_set_active_profile(fw_runtime_t *rt, uint32_t profile_idx);

/* Runs one Hall-effect evaluation pass over raw_adc[160], producing key_bitmap + HID report. */
void fw_runtime_process_scan(fw_runtime_t *rt, const uint16_t raw_adc[FW_NUM_SENSORS], fw_hid_report_t *out_hid);

/* Builds 139-byte MSG_UPDATE frame and 401-byte MSG_QC_SENSOR_DATA frame. */
void fw_runtime_build_msg_update(fw_runtime_t *rt, uint32_t timestamp_ms, uint8_t out_buf[FW_UART_MSG_UPDATE_LEN]);
void fw_runtime_build_msg_qc(fw_runtime_t *rt, uint8_t out_buf[FW_UART_MSG_QC_LEN]);

/* Handles 4-byte UART ACK/response from the SoC bridge: [msg_type, error, qc_mode, stress_test]. */
void fw_runtime_handle_uart_response(fw_runtime_t *rt, const uint8_t *payload, size_t len);

#endif /* FW_SCANNER_H */
