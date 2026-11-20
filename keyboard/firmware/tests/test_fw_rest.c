#include "fw_keyframe.h"
#include "fw_rest.h"
#include "fw_rest_runtime.h"
#include "lfs_store.h"

#include <assert.h>
#include <string.h>

static void test_frame_changes(void) {
    fw_keyframe_detector_t d = {0};
    uint16_t raw[112];
    for (unsigned i = 0; i < 112; ++i)
        raw[i] = 2048;
    raw[5] = 2600;
    raw[106] = 1350;
    for (uint32_t ms = 1; ms <= 1001; ms++)
        assert(!fw_keyframe_poll(&d, raw, ms).attached); /* IDs alone are not a seated frame */
    for (unsigned i = 0; i < 5; ++i) {
        raw[i] = 2300;
        raw[107 + i] = 1750;
    }
    unsigned requests = 0;
    for (uint32_t ms = 1002; ms <= 5000; ms++) {
        fw_keyframe_result_t f = fw_keyframe_poll(&d, raw, ms);
        requests += f.calibrate;
        if (f.attached)
            assert(f.layout == 1 && f.feel == 2);
    }
    assert(requests == 1 && d.frame.attached);
    for (unsigned i = 0; i < 5; i++)
        raw[i] = raw[107 + i] = 2048;
    assert(fw_keyframe_poll(&d, raw, 5201).attached);
    assert(fw_keyframe_poll(&d, raw, 5301).attached);
    assert(fw_keyframe_poll(&d, raw, 5401).attached);
    fw_keyframe_result_t f = fw_keyframe_poll(&d, raw, 5501);
    assert(!f.attached && !f.layout && !f.feel && f.changed && !f.calibrate);
}

static void test_rest_files(void) {
    int16_t original[FW_REST_CHANNELS], decoded[FW_REST_CHANNELS];
    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        original[i] = (int16_t)(2050 + i);
    uint8_t record[FW_REST_BYTES];
    assert(fw_rest_encode(1, 2, original, record) == 0);
    const uint8_t checksum[4] = {0x90, 0x61, 0x85, 0x3d};
    assert(memcmp(record + 232, checksum, 4) == 0);
    assert(fw_rest_decode(record, 1, 2, decoded) == 0 && memcmp(original, decoded, sizeof decoded) == 0);
    assert(fw_rest_decode(record, 1, 1, decoded) != 0);
    const uint8_t legacy[4] = {0x44, 0xf5, 0xf8, 0x6f};
    memcpy(record + 232, legacy, 4);
    assert(fw_rest_decode(record, 1, 2, decoded) == 0 && memcmp(original, decoded, sizeof decoded) == 0);
    assert(fw_rest_decode(record, 1, 1, decoded) != 0 && fw_rest_decode(record, 2, 2, decoded) != 0);
    record[120] ^= 1;
    assert(fw_rest_decode(record, 1, 2, decoded) != 0);

    assert(fw_lfs_host_format_and_mount() == 0);
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    rt.keyframe_attached = 1;
    rt.keyframe_layout = 1;
    rt.switch_feel = 2;
    memcpy(rt.rest_ref_adc, original, sizeof original);
    rt.rest_ref_valid = 1;
    assert(fw_lfs_save_rest_ref(&rt) == 0);
    rt.switch_feel = 1;
    assert(fw_lfs_load_rest_ref(&rt) != 0 && !rt.rest_ref_valid);
    rt.switch_feel = 2;
    rt.keyframe_layout = 2;
    assert(fw_lfs_load_rest_ref(&rt) != 0 && !rt.rest_ref_valid);
    rt.keyframe_layout = 1;
    assert(fw_lfs_load_rest_ref(&rt) == 0 && rt.rest_ref_valid &&
           memcmp(rt.rest_ref_adc, original, sizeof original) == 0);
    rt.keyframe_attached = 0;
    assert(fw_lfs_load_rest_ref(&rt) != 0 && !rt.rest_ref_valid);
}

static void test_boot_and_release(void) {
    uint16_t mean[FW_NUM_SENSORS];
    int16_t reference[FW_REST_CHANNELS];
    uint8_t unstable[FW_REST_CHANNELS] = {0};
    for (unsigned i = 0; i < FW_NUM_SENSORS; i++)
        mean[i] = 2048;
    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        reference[i] = 2048;
    mean[6] += 380;
    mean[13] -= 380;
    fw_rest_guard_t guard;
    fw_rest_apply(1, mean, unstable, reference, &guard);
    assert(guard.match == FW_REST_MATCH && guard.off_keys == 1 && guard.key[0][0] == FW_REST_HELD);
    assert(mean[6] == 2048 && mean[13] == 2048);
    fw_rest_session_t session;
    fw_rest_begin(&session, 1, &guard, true, false, 0, 15);
    assert(session.inhibit_all_new && session.inhibit[0][0]);
    uint16_t raw[FW_NUM_SENSORS];
    memcpy(raw, mean, sizeof mean);
    int32_t delta[FW_NUM_SENSORS] = {0};
    uint8_t down[6][16] = {{0}}, pending[6][16] = {{0}};
    fw_rest_recover(&session, 16, raw, delta, down, pending, false, 0, mean);
    fw_rest_recover(&session, 115, raw, delta, down, pending, false, 0, mean);
    assert(session.inhibit[0][0]);
    fw_rest_recover(&session, 116, raw, delta, down, pending, false, 0, mean);
    assert(!session.inhibit_all_new && !session.inhibit[0][0]);
    unstable[6] = 1;
    fw_rest_apply(1, mean, unstable, NULL, &guard);
    assert(guard.match == FW_REST_NO_REFERENCE && guard.key[0][0] == FW_REST_UNCERTAIN);

    fw_rest_begin(&session, 1, &guard, true, false, 0, 0);
    for (unsigned k = 0; k < 8; k++) {
        raw[6] = (uint16_t)(2050 + k);
        assert(!fw_rest_idle(&session, 480000 + k * 120000, false, raw, mean));
    }
    raw[6] = 3000;
    assert(fw_rest_idle(&session, 1440000, false, raw, mean));
    assert(mean[6] == 2054);
    assert(!fw_rest_idle(&session, 1560000, false, raw, mean));
}

/* The 50 ms inhibit after relearning an Esc sensor pair must cross timer wrap without
 * either suppressing unrelated keys or returning weeks after the inhibit has expired. */
static void test_long_running_clock(uint32_t start) {
    fw_rest_guard_t g = {.match = FW_REST_MATCH};
    g.key[0][0] = FW_REST_HELD;
    fw_rest_session_t session;
    fw_rest_begin(&session, 1, &g, true, false, 0, start);
    uint16_t raw[FW_NUM_SENSORS], baseline[FW_NUM_SENSORS];
    for (unsigned i = 0; i < FW_NUM_SENSORS; i++)
        raw[i] = baseline[i] = 2100;
    uint8_t up[6][16] = {{0}};
    int32_t delta[FW_NUM_SENSORS] = {0};
    const unsigned times[] = {0, 100, 101, 201, 202, 250, 251, 0x80000020U, 0xffffffe0U, 48, 201};
    for (unsigned i = 0; i < sizeof times / sizeof times[0]; i++) {
        fw_rest_recover(&session, start + times[i], raw, delta, up, up, false, 0, baseline);
        bool esc = i == 0 || times[i] == 202 || times[i] == 250;
        assert(session.inhibit[0][0] == esc && !session.inhibit[5][10]);
    }
}

/* A frame attaches with Esc held. The bridge distinguishes ordinary attachment from
 * temporary recovery, without reporting a second attachment when the key is released. */
static void test_recovery_phase(void) {
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    uint16_t raw[FW_NUM_SENSORS];
    for (unsigned i = 0; i < FW_NUM_SENSORS; i++)
        raw[i] = 2048;
    raw[5] = 2600;
    raw[106] = 1350;
    for (unsigned i = 0; i < 5; ++i) {
        raw[i] = 2300;
        raw[107 + i] = 1750;
    }
    unsigned attachments = 0;
    for (unsigned ms = 1; ms <= 1001; ms += 100)
        attachments += fw_runtime_poll_frame(&rt, raw, ms);
    assert(attachments == 1 && rt.keyframe_layout == 1 && rt.switch_feel == 2);
    for (unsigned i = 0; i < FW_REST_CHANNELS; i++)
        rt.rest_ref_adc[i] = (int16_t)raw[i];
    rt.rest_ref_valid = 1;
    raw[6] += 380;
    raw[13] -= 380;
    uint8_t unstable[FW_REST_CHANNELS] = {0};
    fw_runtime_calibrate(&rt, raw, unstable, 1001, true);
    uint8_t message[FW_UART_MSG_UPDATE_LEN];
    assert(!fw_runtime_poll_frame(&rt, raw, 1101));
    fw_runtime_build_msg_update(&rt, 1101, message);
    assert(message[5 + 0x25] == 1 && message[5 + 0x6a] == 4);
    raw[6] = raw[13] = 2048;
    fw_hid_report_t hid;
    for (unsigned ms = 1102; ms < 1402; ms++) {
        assert(!fw_runtime_poll_frame(&rt, raw, ms));
        fw_runtime_process_scan(&rt, raw, &hid);
        assert(!fw_runtime_finish_scan(&rt, ms));
    }
    fw_runtime_build_msg_update(&rt, 1402, message);
    assert(message[5 + 0x25] == 1 && message[5 + 0x6a] == 3);
}

void test_fw_rest(void) {
    test_recovery_phase();
    test_long_running_clock(0x80000001U);
    test_long_running_clock(UINT32_MAX - 225U);
    test_frame_changes();
    test_rest_files();
    test_boot_and_release();
}
