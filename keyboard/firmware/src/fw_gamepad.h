#ifndef FW_GAMEPAD_H
#define FW_GAMEPAD_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/*
 * Hall-effect Analog Gamepad evaluator (Wooting/Keychron-style).
 *
 * Pure portable C — reads calibrated Hall travel (`flux_um[k]`, 0..2800 um) for every
 * physical key whose active QMK layer resolves to a fixed GP_* keycode (0x7E27..0x7E3F),
 * applies deadzone, saturation, response curve, and optional circular radial clamp, and
 * emits a 15-byte Classic USB HID Gamepad report at 1000 Hz.
 */

#define FW_GP_KEYCODE_BASE  0x7E27U
#define FW_GP_KEYCODE_END   0x7E3FU
#define FW_GP_KEYCODE_COUNT 25U

enum fw_gamepad_keycode {
    FW_GP_LS_UP      = 0x7E27U,
    FW_GP_LS_DOWN    = 0x7E28U,
    FW_GP_LS_LEFT    = 0x7E29U,
    FW_GP_LS_RIGHT   = 0x7E2AU,
    FW_GP_RS_UP      = 0x7E2BU,
    FW_GP_RS_DOWN    = 0x7E2CU,
    FW_GP_RS_LEFT    = 0x7E2DU,
    FW_GP_RS_RIGHT   = 0x7E2EU,
    FW_GP_LT         = 0x7E2FU,
    FW_GP_RT         = 0x7E30U,
    FW_GP_A          = 0x7E31U,
    FW_GP_B          = 0x7E32U,
    FW_GP_X          = 0x7E33U,
    FW_GP_Y          = 0x7E34U,
    FW_GP_LB         = 0x7E35U,
    FW_GP_RB         = 0x7E36U,
    FW_GP_L3         = 0x7E37U,
    FW_GP_R3         = 0x7E38U,
    FW_GP_DPAD_UP    = 0x7E39U,
    FW_GP_DPAD_DOWN  = 0x7E3AU,
    FW_GP_DPAD_LEFT  = 0x7E3BU,
    FW_GP_DPAD_RIGHT = 0x7E3CU,
    FW_GP_START      = 0x7E3DU,
    FW_GP_BACK       = 0x7E3EU,
    FW_GP_GUIDE      = 0x7E3FU,
};

enum fw_gamepad_mode {
    FW_GP_MODE_OFF    = 0U, /* No gamepad USB interface enumerated */
    FW_GP_MODE_HID    = 1U, /* Classic USB HID Gamepad (macOS / Linux / Steam native) */
    FW_GP_MODE_XINPUT = 2U, /* Reserved for Phase 2 XInput (enumerates HID for now) */
};

enum fw_gamepad_curve {
    FW_GP_CURVE_LINEAR    = 0U,
    FW_GP_CURVE_SCURVE    = 1U, /* Smooth S-curve: 3t^2 - 2t^3 */
    FW_GP_CURVE_SNAPPY    = 2U, /* Fast response: 1 - (1 - t)^2 */
    FW_GP_CURVE_PRECISION = 3U, /* Fine center control: t^2 */
};

#define FW_GP_CONFIG_WIRE_LEN 7U
#define FW_GP_REPORT_WIRE_LEN 15U

typedef struct {
    uint8_t mode;
    uint16_t deadzone_um;
    uint16_t saturation_um;
    uint8_t curve;
    bool radial_clamp;
} fw_gamepad_config_t;

typedef struct {
    int16_t lx;       /* Left stick X: -32767..+32767 */
    int16_t ly;       /* Left stick Y: -32767..+32767 (up = negative, down = positive) */
    int16_t rx;       /* Right stick X */
    int16_t ry;       /* Right stick Y */
    uint16_t lt;      /* Left trigger: 0..65535 */
    uint16_t rt;      /* Right trigger: 0..65535 */
    uint8_t hat;      /* 0=N, 1=NE, 2=E, 3=SE, 4=S, 5=SW, 6=W, 7=NW, 8=centered */
    uint16_t buttons; /* Bits 0..10 = A, B, X, Y, LB, RB, L3, R3, Start, Back, Guide */
} fw_gamepad_report_t;

void fw_gamepad_config_default(fw_gamepad_config_t *cfg);
int fw_gamepad_config_decode(const uint8_t *buf, size_t len, fw_gamepad_config_t *out);
void fw_gamepad_config_encode(const fw_gamepad_config_t *cfg, uint8_t out[FW_GP_CONFIG_WIRE_LEN]);

/* Evaluate 1000 Hz analog gamepad axes, triggers, D-Pad hat, and buttons from Hall travel. */
void fw_gamepad_evaluate(const fw_gamepad_config_t *cfg, const uint16_t *flux_um, const uint8_t *key_pressed,
                         const uint16_t *resolved_kc, uint16_t n_keys, fw_gamepad_report_t *out);

/* Serialize report into the 15-byte Classic USB HID Gamepad wire payload. */
void fw_gamepad_report_pack(const fw_gamepad_report_t *rep, uint8_t out[FW_GP_REPORT_WIRE_LEN]);

#endif /* FW_GAMEPAD_H */
