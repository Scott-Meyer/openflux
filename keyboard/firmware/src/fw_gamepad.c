#include "fw_gamepad.h"

#include <string.h>

void fw_gamepad_config_default(fw_gamepad_config_t *cfg) {
    if (!cfg)
        return;
    cfg->mode = FW_GP_MODE_OFF;
    cfg->deadzone_um = 200U;
    cfg->saturation_um = 2600U;
    cfg->curve = FW_GP_CURVE_LINEAR;
    cfg->radial_clamp = true;
}

static void sanitize_config(fw_gamepad_config_t *cfg) {
    if (cfg->mode > FW_GP_MODE_XINPUT)
        cfg->mode = FW_GP_MODE_OFF;
    if (cfg->deadzone_um > 2400U)
        cfg->deadzone_um = 2400U;
    if (cfg->saturation_um < (uint16_t)(cfg->deadzone_um + 100U))
        cfg->saturation_um = (uint16_t)(cfg->deadzone_um + 100U);
    if (cfg->saturation_um > 2800U)
        cfg->saturation_um = 2800U;
    if (cfg->curve > FW_GP_CURVE_PRECISION)
        cfg->curve = FW_GP_CURVE_LINEAR;
}

int fw_gamepad_config_decode(const uint8_t *buf, size_t len, fw_gamepad_config_t *out) {
    if (!buf || !out || len < FW_GP_CONFIG_WIRE_LEN)
        return -1;
    out->mode = buf[0];
    out->deadzone_um = (uint16_t)buf[1] | ((uint16_t)buf[2] << 8);
    out->saturation_um = (uint16_t)buf[3] | ((uint16_t)buf[4] << 8);
    out->curve = buf[5];
    out->radial_clamp = (buf[6] & 0x01U) != 0U;
    sanitize_config(out);
    return 0;
}

void fw_gamepad_config_encode(const fw_gamepad_config_t *cfg, uint8_t out[FW_GP_CONFIG_WIRE_LEN]) {
    if (!cfg || !out)
        return;
    out[0] = cfg->mode;
    out[1] = (uint8_t)(cfg->deadzone_um & 0xFFU);
    out[2] = (uint8_t)(cfg->deadzone_um >> 8);
    out[3] = (uint8_t)(cfg->saturation_um & 0xFFU);
    out[4] = (uint8_t)(cfg->saturation_um >> 8);
    out[5] = cfg->curve;
    out[6] = cfg->radial_clamp ? 1U : 0U;
}

/* Map raw travel um -> normalized Q15 (0..32767) with deadzone, saturation, and response curve. */
static uint16_t map_analog_q15(uint16_t flux_um, const fw_gamepad_config_t *cfg) {
    if (flux_um <= cfg->deadzone_um)
        return 0U;
    if (flux_um >= cfg->saturation_um)
        return 32767U;
    uint32_t span = (uint32_t)(cfg->saturation_um - cfg->deadzone_um);
    if (span == 0U)
        return 32767U;
    uint32_t t = ((uint32_t)(flux_um - cfg->deadzone_um) * 32767U) / span; /* 0..32767 */

    switch (cfg->curve) {
        case FW_GP_CURVE_SCURVE: {
            /* 3*t^2 - 2*t^3 in Q15 */
            uint32_t t2 = (t * t) / 32767U;
            uint32_t t3 = (t2 * t) / 32767U;
            uint32_t v = (3U * t2 > 2U * t3) ? (3U * t2 - 2U * t3) : 0U;
            return (v > 32767U) ? 32767U : (uint16_t)v;
        }
        case FW_GP_CURVE_SNAPPY: {
            /* 1 - (1 - t)^2 */
            uint32_t inv = 32767U - t;
            uint32_t inv2 = (inv * inv) / 32767U;
            return (uint16_t)(32767U - inv2);
        }
        case FW_GP_CURVE_PRECISION: {
            /* t^2 */
            return (uint16_t)((t * t) / 32767U);
        }
        case FW_GP_CURVE_LINEAR:
        default:
            return (uint16_t)t;
    }
}

static uint32_t isqrt_u64(uint64_t n) {
    uint64_t x = n;
    uint64_t y = (x + 1ULL) >> 1;
    while (y < x) {
        x = y;
        y = (x + n / x) >> 1;
    }
    return (uint32_t)x;
}

static void clamp_stick(int32_t *x, int32_t *y, bool radial_clamp) {
    if (radial_clamp) {
        int64_t xx = (int64_t)(*x) * (int64_t)(*x);
        int64_t yy = (int64_t)(*y) * (int64_t)(*y);
        uint64_t mag2 = (uint64_t)(xx + yy);
        const uint64_t max2 = 32767ULL * 32767ULL;
        if (mag2 > max2) {
            uint32_t mag = isqrt_u64(mag2);
            if (mag > 0U) {
                *x = (int32_t)(((int64_t)(*x) * 32767LL) / (int64_t)mag);
                *y = (int32_t)(((int64_t)(*y) * 32767LL) / (int64_t)mag);
            }
        }
    }
    if (*x > 32767)
        *x = 32767;
    if (*x < -32767)
        *x = -32767;
    if (*y > 32767)
        *y = 32767;
    if (*y < -32767)
        *y = -32767;
}

static uint8_t dpad_to_hat(bool up, bool down, bool left, bool right) {
    if (up && down)
        up = down = false;
    if (left && right)
        left = right = false;
    if (up && right)
        return 1U;
    if (right && down)
        return 3U;
    if (down && left)
        return 5U;
    if (left && up)
        return 7U;
    if (up)
        return 0U;
    if (right)
        return 2U;
    if (down)
        return 4U;
    if (left)
        return 6U;
    return 8U; /* centered / null state */
}

void fw_gamepad_evaluate(const fw_gamepad_config_t *cfg, const uint16_t *flux_um, const uint8_t *key_pressed,
                         const uint16_t *resolved_kc, uint16_t n_keys, fw_gamepad_report_t *out) {
    if (!out)
        return;
    memset(out, 0, sizeof(*out));
    out->hat = 8U;
    if (!cfg || cfg->mode == FW_GP_MODE_OFF || !flux_um || !resolved_kc)
        return;

    uint16_t ls_up = 0, ls_dn = 0, ls_lt = 0, ls_rt = 0;
    uint16_t rs_up = 0, rs_dn = 0, rs_lt = 0, rs_rt = 0;
    uint16_t trig_l = 0, trig_r = 0;
    bool d_up = false, d_dn = false, d_lt = false, d_rt = false;
    uint16_t buttons = 0U;

    for (uint16_t k = 0U; k < n_keys; k++) {
        uint16_t kc = resolved_kc[k];
        if (kc < FW_GP_KEYCODE_BASE || kc > FW_GP_KEYCODE_END)
            continue;
        uint16_t q15 = map_analog_q15(flux_um[k], cfg);
        bool down = (key_pressed && key_pressed[k] != 0U) || q15 >= 16384U;

        switch (kc) {
            case FW_GP_LS_UP:    if (q15 > ls_up) ls_up = q15; break;
            case FW_GP_LS_DOWN:  if (q15 > ls_dn) ls_dn = q15; break;
            case FW_GP_LS_LEFT:  if (q15 > ls_lt) ls_lt = q15; break;
            case FW_GP_LS_RIGHT: if (q15 > ls_rt) ls_rt = q15; break;
            case FW_GP_RS_UP:    if (q15 > rs_up) rs_up = q15; break;
            case FW_GP_RS_DOWN:  if (q15 > rs_dn) rs_dn = q15; break;
            case FW_GP_RS_LEFT:  if (q15 > rs_lt) rs_lt = q15; break;
            case FW_GP_RS_RIGHT: if (q15 > rs_rt) rs_rt = q15; break;
            case FW_GP_LT:       if (q15 > trig_l) trig_l = q15; break;
            case FW_GP_RT:       if (q15 > trig_r) trig_r = q15; break;
            case FW_GP_A:        if (down) buttons |= (1U << 0); break;
            case FW_GP_B:        if (down) buttons |= (1U << 1); break;
            case FW_GP_X:        if (down) buttons |= (1U << 2); break;
            case FW_GP_Y:        if (down) buttons |= (1U << 3); break;
            case FW_GP_LB:       if (down) buttons |= (1U << 4); break;
            case FW_GP_RB:       if (down) buttons |= (1U << 5); break;
            case FW_GP_L3:       if (down) buttons |= (1U << 6); break;
            case FW_GP_R3:       if (down) buttons |= (1U << 7); break;
            case FW_GP_START:    if (down) buttons |= (1U << 8); break;
            case FW_GP_BACK:     if (down) buttons |= (1U << 9); break;
            case FW_GP_GUIDE:    if (down) buttons |= (1U << 10); break;
            case FW_GP_DPAD_UP:    if (down) d_up = true; break;
            case FW_GP_DPAD_DOWN:  if (down) d_dn = true; break;
            case FW_GP_DPAD_LEFT:  if (down) d_lt = true; break;
            case FW_GP_DPAD_RIGHT: if (down) d_rt = true; break;
            default: break;
        }
    }

    int32_t lx = (int32_t)ls_rt - (int32_t)ls_lt;
    int32_t ly = (int32_t)ls_dn - (int32_t)ls_up;
    int32_t rx = (int32_t)rs_rt - (int32_t)rs_lt;
    int32_t ry = (int32_t)rs_dn - (int32_t)rs_up;
    clamp_stick(&lx, &ly, cfg->radial_clamp);
    clamp_stick(&rx, &ry, cfg->radial_clamp);

    out->lx = (int16_t)lx;
    out->ly = (int16_t)ly;
    out->rx = (int16_t)rx;
    out->ry = (int16_t)ry;
    /* Scale Q15 (0..32767) -> full 16-bit unsigned trigger (0..65535). */
    out->lt = (trig_l == 32767U) ? 65535U : (uint16_t)((uint32_t)trig_l << 1);
    out->rt = (trig_r == 32767U) ? 65535U : (uint16_t)((uint32_t)trig_r << 1);
    out->hat = dpad_to_hat(d_up, d_dn, d_lt, d_rt);
    out->buttons = buttons;
}

static void wr_le16_gp(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)(v & 0xFFU);
    p[1] = (uint8_t)(v >> 8);
}

void fw_gamepad_report_pack(const fw_gamepad_report_t *rep, uint8_t out[FW_GP_REPORT_WIRE_LEN]) {
    if (!rep || !out)
        return;
    wr_le16_gp(out + 0U, (uint16_t)rep->lx);
    wr_le16_gp(out + 2U, (uint16_t)rep->ly);
    wr_le16_gp(out + 4U, (uint16_t)rep->rx);
    wr_le16_gp(out + 6U, (uint16_t)rep->ry);
    wr_le16_gp(out + 8U, rep->lt);
    wr_le16_gp(out + 10U, rep->rt);
    out[12] = rep->hat;
    wr_le16_gp(out + 13U, rep->buttons);
}
