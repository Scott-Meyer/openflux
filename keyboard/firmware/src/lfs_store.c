#include "lfs_store.h"
#include "fw_rest.h"
#include "lfs.h"

#ifndef HOST_TEST
#include "hardware/flash.h"
#include "hardware/sync.h"
#include "pico/stdlib.h"
#endif

#include <string.h>

#ifdef HOST_TEST
static uint8_t g_host_flash[FW_LFS_BLOCK_COUNT * FW_LFS_BLOCK_SIZE];
static int g_fail_program_after = -1;
void fw_lfs_host_fail_program_after(int successful_writes) {
    g_fail_program_after = successful_writes;
}
void fw_lfs_host_reset_flash(void) {
    g_fail_program_after = -1;
    memset(g_host_flash, 0xFF, sizeof(g_host_flash));
}
static int rp2040_lfs_read(const struct lfs_config *c, lfs_block_t block, lfs_off_t off, void *buffer,
                           lfs_size_t size) {
    (void)c;
    if (block >= FW_LFS_BLOCK_COUNT || off + size > FW_LFS_BLOCK_SIZE)
        return LFS_ERR_IO;
    memcpy(buffer, g_host_flash + (size_t)block * FW_LFS_BLOCK_SIZE + off, size);
    return LFS_ERR_OK;
}
static int rp2040_lfs_prog(const struct lfs_config *c, lfs_block_t block, lfs_off_t off, const void *buffer,
                           lfs_size_t size) {
    (void)c;
    if (block >= FW_LFS_BLOCK_COUNT || off + size > FW_LFS_BLOCK_SIZE)
        return LFS_ERR_IO;
    if (g_fail_program_after == 0)
        return LFS_ERR_IO;
    if (g_fail_program_after > 0)
        --g_fail_program_after;
    memcpy(g_host_flash + (size_t)block * FW_LFS_BLOCK_SIZE + off, buffer, size);
    return LFS_ERR_OK;
}
static int rp2040_lfs_erase(const struct lfs_config *c, lfs_block_t block) {
    (void)c;
    if (block >= FW_LFS_BLOCK_COUNT)
        return LFS_ERR_IO;
    memset(g_host_flash + (size_t)block * FW_LFS_BLOCK_SIZE, 0xFF, FW_LFS_BLOCK_SIZE);
    return LFS_ERR_OK;
}
#define FW_FLASH_PAGE_SIZE 256U
#else
#define FW_FLASH_PAGE_SIZE FLASH_PAGE_SIZE
static int rp2040_lfs_read(const struct lfs_config *c, lfs_block_t block, lfs_off_t off, void *buffer,
                           lfs_size_t size) {
    (void)c;
    if (block >= FW_LFS_BLOCK_COUNT || off + size > FW_LFS_BLOCK_SIZE)
        return LFS_ERR_IO;
    const uint8_t *xip =
        (const uint8_t *)(XIP_NOCACHE_NOALLOC_BASE + FW_LFS_OFFSET_BYTES + block * FW_LFS_BLOCK_SIZE + off);
    memcpy(buffer, xip, size);
    return LFS_ERR_OK;
}

static int rp2040_lfs_prog(const struct lfs_config *c, lfs_block_t block, lfs_off_t off, const void *buffer,
                           lfs_size_t size) {
    (void)c;
    if (block >= FW_LFS_BLOCK_COUNT || off + size > FW_LFS_BLOCK_SIZE)
        return LFS_ERR_IO;
    uint32_t addr = FW_LFS_OFFSET_BYTES + block * FW_LFS_BLOCK_SIZE + off;
    uint32_t irq = save_and_disable_interrupts();
    flash_range_program(addr, (const uint8_t *)buffer, size);
    restore_interrupts(irq);
    return LFS_ERR_OK;
}

static int rp2040_lfs_erase(const struct lfs_config *c, lfs_block_t block) {
    (void)c;
    if (block >= FW_LFS_BLOCK_COUNT)
        return LFS_ERR_IO;
    uint32_t addr = FW_LFS_OFFSET_BYTES + block * FW_LFS_BLOCK_SIZE;
    uint32_t irq = save_and_disable_interrupts();
    flash_range_erase(addr, FW_LFS_BLOCK_SIZE);
    restore_interrupts(irq);
    return LFS_ERR_OK;
}
#endif

static int rp2040_lfs_sync(const struct lfs_config *c) {
    (void)c;
    return LFS_ERR_OK;
}

static uint8_t g_read_buf[FW_LFS_BLOCK_SIZE];
static uint8_t g_prog_buf[FW_LFS_BLOCK_SIZE];
static uint8_t g_lookahead_buf[16];

/* Explicit 256-block and 255-block mount configs (never block_count=0, avoiding seed % 0 on corrupt superblocks). */
static const struct lfs_config g_lfs_cfg_256 = {
    .read = rp2040_lfs_read,
    .prog = rp2040_lfs_prog,
    .erase = rp2040_lfs_erase,
    .sync = rp2040_lfs_sync,
    .read_size = 1,
    .prog_size = FW_FLASH_PAGE_SIZE,
    .block_size = FW_LFS_BLOCK_SIZE,
    .block_count = FW_LFS_BLOCK_COUNT,
    .cache_size = FW_LFS_BLOCK_SIZE,
    .lookahead_size = 16,
    .block_cycles = 500,
    .read_buffer = g_read_buf,
    .prog_buffer = g_prog_buf,
    .lookahead_buffer = g_lookahead_buf,
};

static const struct lfs_config g_lfs_cfg_255 = {
    .read = rp2040_lfs_read,
    .prog = rp2040_lfs_prog,
    .erase = rp2040_lfs_erase,
    .sync = rp2040_lfs_sync,
    .read_size = 1,
    .prog_size = FW_FLASH_PAGE_SIZE,
    .block_size = FW_LFS_BLOCK_SIZE,
    .block_count = 255U,
    .cache_size = FW_LFS_BLOCK_SIZE,
    .lookahead_size = 16,
    .block_cycles = 500,
    .read_buffer = g_read_buf,
    .prog_buffer = g_prog_buf,
    .lookahead_buffer = g_lookahead_buf,
};

static lfs_t g_lfs;
static uint8_t g_mounted = 0;

#ifdef HOST_TEST
int fw_lfs_host_format_and_mount(void) {
    fw_lfs_host_reset_flash();
    if (lfs_format(&g_lfs, &g_lfs_cfg_256) != LFS_ERR_OK)
        return -1;
    if (lfs_mount(&g_lfs, &g_lfs_cfg_256) != LFS_ERR_OK)
        return -1;
    g_mounted = 1U;
    return 0;
}

int fw_lfs_host_format_blocks_and_mount(uint32_t block_count) {
    fw_lfs_host_reset_flash();
    const struct lfs_config *cfg = (block_count == 255U) ? &g_lfs_cfg_255 : &g_lfs_cfg_256;
    if (block_count != 255U && block_count != FW_LFS_BLOCK_COUNT)
        return -1;
    if (lfs_format(&g_lfs, cfg) != LFS_ERR_OK)
        return -1;
    (void)memset(&g_lfs, 0, sizeof(g_lfs));
    return fw_lfs_mount();
}

int fw_lfs_host_unmount_and_remount(void) {
    if (g_mounted) {
        (void)lfs_unmount(&g_lfs);
        g_mounted = 0U;
    }
    memset(&g_lfs, 0, sizeof(g_lfs));
    return fw_lfs_mount();
}
#endif

int fw_lfs_mount(void) {
    /* Never auto-format on mount error so factory calibration is never wiped. */
    if (lfs_mount(&g_lfs, &g_lfs_cfg_256) == LFS_ERR_OK || lfs_mount(&g_lfs, &g_lfs_cfg_255) == LFS_ERR_OK) {
        g_mounted = 1U;
        return 0;
    }
    g_mounted = 0U;
    return -1;
}

static int lfs_read_exact(const char *name, uint8_t *buf, size_t max_len, size_t *out_len) {
    if (!g_mounted)
        return -1;
    lfs_file_t f;
    if (lfs_file_open(&g_lfs, &f, name, LFS_O_RDONLY) != LFS_ERR_OK) {
        return -1;
    }
    lfs_ssize_t sz = lfs_file_size(&g_lfs, &f);
    if (sz < 0 || (size_t)sz > max_len) {
        lfs_file_close(&g_lfs, &f);
        return -1;
    }
    lfs_ssize_t rd = lfs_file_read(&g_lfs, &f, buf, (lfs_size_t)sz);
    lfs_file_close(&g_lfs, &f);
    if (rd != sz)
        return -1;
    if (out_len)
        *out_len = (size_t)rd;
    return 0;
}

static int lfs_write_file(const char *name, const uint8_t *buf, size_t len) {
    if (!g_mounted)
        return -1;
    /* Refuse any write to factory calibration files. */
    if (strcmp(name, "factory_calibration_info") == 0 || strcmp(name, "factory_scaling_factors") == 0) {
        return -1;
    }
    lfs_file_t f;
    if (lfs_file_open(&g_lfs, &f, name, LFS_O_WRONLY | LFS_O_CREAT | LFS_O_TRUNC) != LFS_ERR_OK) {
        return -1;
    }
    lfs_ssize_t wr = lfs_file_write(&g_lfs, &f, buf, (lfs_size_t)len);
    int rc = lfs_file_close(&g_lfs, &f);
    return (wr == (lfs_ssize_t)len && rc == LFS_ERR_OK) ? 0 : -1;
}

void fw_lfs_load_boot_state(fw_runtime_t *rt) {
    if (!rt || !g_mounted)
        return;

    uint8_t info[20];
    uint8_t factors[FW_NUM_SENSORS * 2U];
    size_t ilen = 0, flen = 0;
    if (lfs_read_exact("factory_calibration_info", info, sizeof(info), &ilen) == 0 &&
        lfs_read_exact("factory_scaling_factors", factors, sizeof(factors), &flen) == 0) {
        (void)fw_calibration_load(&rt->cal, info, ilen, factors, flen);
    }

    static uint8_t km_tmp[FW_MAX_KEYMAP_BYTES];
    size_t km_len = 0;
    if (lfs_read_exact("keymap", km_tmp, sizeof(km_tmp), &km_len) == 0) {
        (void)fw_runtime_load_keymap(rt, km_tmp, km_len);
    }

    uint8_t ps_tmp[32 + FW_MAX_PROFILES * 8U];
    size_t ps_len = 0;
    if (lfs_read_exact("profile_settings_v1", ps_tmp + 16U, sizeof(ps_tmp) - 16U, &ps_len) == 0 && ps_len >= 16U) {
        /* Prepend synthetic v1 FileHeader (type=0) so boot uses the same validator/applier as USB updates. */
        memset(ps_tmp, 0, 16U);
        (void)fw_runtime_load_profile_settings(rt, ps_tmp, 16U + ps_len);
    }

    uint8_t u32_tmp[4];
    size_t u32_len = 0;
    if (lfs_read_exact("language_code", u32_tmp, sizeof(u32_tmp), &u32_len) == 0 && u32_len == 4U) {
        rt->language_code = (uint32_t)u32_tmp[0] | ((uint32_t)u32_tmp[1] << 8) | ((uint32_t)u32_tmp[2] << 16) |
                            ((uint32_t)u32_tmp[3] << 24);
    }
    if (lfs_read_exact("platform_type_code", u32_tmp, sizeof(u32_tmp), &u32_len) == 0 && u32_len == 4U) {
        rt->platform_code = (uint32_t)u32_tmp[0] | ((uint32_t)u32_tmp[1] << 8) | ((uint32_t)u32_tmp[2] << 16) |
                            ((uint32_t)u32_tmp[3] << 24);
    }
}

static void rest_ref_filename(uint8_t layout, uint8_t feel, char out[40]) {
    const char *prefix = "rest_reference_";
    size_t i = 0;
    while (prefix[i]) {
        out[i] = prefix[i];
        i++;
    }
    out[i++] = (char)('0' + (layout % 10U));
    out[i++] = '_';
    out[i++] = (char)('0' + (feel % 10U));
    out[i] = '\0';
}

int fw_lfs_save_rest_ref(const fw_runtime_t *rt) {
    if (!rt || !rt->keyframe_attached || !rt->rest_ref_valid)
        return -1;
    uint8_t buf[FW_REST_BYTES];
    if (fw_rest_encode(rt->keyframe_layout, rt->switch_feel, rt->rest_ref_adc, buf) != 0)
        return -1;
    char fname[40];
    rest_ref_filename(rt->keyframe_layout, rt->switch_feel, fname);
    return lfs_write_file(fname, buf, sizeof(buf));
}

int fw_lfs_load_rest_ref(fw_runtime_t *rt) {
    if (!rt)
        return -1;
    rt->rest_ref_valid = 0U;
    if (!rt->keyframe_attached)
        return -1;
    char fname[40];
    rest_ref_filename(rt->keyframe_layout, rt->switch_feel, fname);
    uint8_t buf[FW_REST_BYTES];
    size_t n = 0;
    if (lfs_read_exact(fname, buf, sizeof(buf), &n) != 0 || n != sizeof(buf)) {
        return -1;
    }
    if (fw_rest_decode(buf, rt->keyframe_layout, rt->switch_feel, rt->rest_ref_adc) != 0) {
        return -1;
    }
    rt->rest_ref_valid = 1U;
    return 0;
}

static const char refinement_file[] = "press_refinement_v1";
static const char refinement_stage[] = "press_refinement_v1.tmp";

fw_refinement_state_t fw_lfs_read_refinement(uint8_t record[FW_REFINEMENT_RECORD_BYTES]) {
    if (!g_mounted || !record)
        return FW_REFINEMENT_UNAVAILABLE;
    lfs_file_t file;
    int rc = lfs_file_open(&g_lfs, &file, refinement_file, LFS_O_RDONLY);
    if (rc == LFS_ERR_NOENT)
        return FW_REFINEMENT_ABSENT;
    if (rc != 0)
        return rc == LFS_ERR_ISDIR ? FW_REFINEMENT_INVALID : FW_REFINEMENT_UNAVAILABLE;

    fw_refinement_state_t state = FW_REFINEMENT_UNAVAILABLE;
    lfs_soff_t size = lfs_file_size(&g_lfs, &file);
    if (size == FW_REFINEMENT_RECORD_BYTES) {
        if (lfs_file_read(&g_lfs, &file, record, FW_REFINEMENT_RECORD_BYTES) == FW_REFINEMENT_RECORD_BYTES)
            state = fw_refinement_validate(record, FW_REFINEMENT_RECORD_BYTES) ? FW_REFINEMENT_INSTALLED
                                                                               : FW_REFINEMENT_INVALID;
    } else if (size >= 0) {
        state = FW_REFINEMENT_INVALID;
    }
    return lfs_file_close(&g_lfs, &file) == 0 ? state : FW_REFINEMENT_UNAVAILABLE;
}

/* Compare without a second full record on the MCU stack. */
static bool refinement_file_matches(const char *name, const uint8_t *record) {
    lfs_file_t file;
    if (lfs_file_open(&g_lfs, &file, name, LFS_O_RDONLY) != 0)
        return false;
    bool match = lfs_file_size(&g_lfs, &file) == FW_REFINEMENT_RECORD_BYTES;
    uint8_t page[64];
    for (size_t i = 0; match && i < FW_REFINEMENT_RECORD_BYTES;) {
        size_t n = FW_REFINEMENT_RECORD_BYTES - i;
        if (n > sizeof(page))
            n = sizeof(page);
        match = lfs_file_read(&g_lfs, &file, page, (lfs_size_t)n) == (lfs_ssize_t)n && memcmp(page, record + i, n) == 0;
        i += n;
    }
    return lfs_file_close(&g_lfs, &file) == 0 && match;
}

int fw_lfs_install_refinement(const uint8_t *record, size_t len) {
    if (!g_mounted || !fw_refinement_validate(record, len))
        return -1;
    if (refinement_file_matches(refinement_file, record))
        return 0;
    /* The only live name is replaced by one LittleFS transaction. A reset before the rename
     * can leave a staging file; it is never loaded at boot and the next write truncates it. */
    if (lfs_write_file(refinement_stage, record, len) == 0 && refinement_file_matches(refinement_stage, record) &&
        lfs_rename(&g_lfs, refinement_stage, refinement_file) == 0)
        return 0;
    (void)lfs_remove(&g_lfs, refinement_stage);
    return -1;
}

int fw_lfs_save_keymap(const uint8_t *blob, size_t len) {
    return lfs_write_file("keymap", blob, len);
}

int fw_lfs_save_profile_settings(const uint8_t *xfer, size_t len) {
    if (!xfer || len < 32U)
        return -1;
    /* Strip 16-byte FileHeader before saving profile_settings_v1 data payload. */
    return lfs_write_file("profile_settings_v1", xfer + 16U, len - 16U);
}

int fw_lfs_save_u32(const char *filename, uint32_t value) {
    uint8_t b[4] = {
        (uint8_t)(value & 0xFFU),
        (uint8_t)((value >> 8) & 0xFFU),
        (uint8_t)((value >> 16) & 0xFFU),
        (uint8_t)((value >> 24) & 0xFFU),
    };
    return lfs_write_file(filename, b, sizeof(b));
}

static void qmk_ee_filenames(uint8_t layout_id, char live[24], char stage[28]) {
    uint8_t lid = (layout_id >= 1U && layout_id <= 3U) ? layout_id : 1U;
    static const char *const k_live[4] = {"vial_ee_1.bin", "vial_ee_1.bin", "vial_ee_2.bin", "vial_ee_3.bin"};
    static const char *const k_stage[4] = {"vial_ee_1.tmp", "vial_ee_1.tmp", "vial_ee_2.tmp", "vial_ee_3.tmp"};
    strcpy(live, k_live[lid]);
    strcpy(stage, k_stage[lid]);
}

/* Versioned 8-byte header for per-layout QMK/Vial EEPROM files: "OFQE" + u16 ver + u16 len. */
static const uint8_t k_qmk_ee_magic[4] = {'O', 'F', 'Q', 'E'};
enum { FW_QMK_EE_SCHEMA_VER = 1U, FW_QMK_EE_HDR_LEN = 8U };

int fw_lfs_load_qmk_eeprom(uint8_t layout_id, uint8_t *buf, size_t len) {
    if (!g_mounted || !buf || len == 0U)
        return -1;
    char live[24], stage[28];
    qmk_ee_filenames(layout_id, live, stage);
    lfs_file_t f;
    int rc = lfs_file_open(&g_lfs, &f, live, LFS_O_RDONLY);
    if (rc == LFS_ERR_NOENT)
        return 1; /* Genuinely absent — safe to seed defaults / legacy migration. */
    if (rc != 0)
        return -1; /* Mount/IO error — caller MUST NOT overwrite user's flash file! */

    lfs_soff_t sz = lfs_file_size(&g_lfs, &f);
    int out_rc = -1;
    if (sz == (lfs_soff_t)(FW_QMK_EE_HDR_LEN + len)) {
        uint8_t hdr[FW_QMK_EE_HDR_LEN];
        if (lfs_file_read(&g_lfs, &f, hdr, sizeof(hdr)) == (lfs_ssize_t)sizeof(hdr)) {
            uint16_t ver = (uint16_t)hdr[4] | ((uint16_t)hdr[5] << 8);
            uint16_t plen = (uint16_t)hdr[6] | ((uint16_t)hdr[7] << 8);
            if (memcmp(hdr, k_qmk_ee_magic, 4U) == 0 && ver == FW_QMK_EE_SCHEMA_VER && (size_t)plen == len &&
                lfs_file_read(&g_lfs, &f, buf, (lfs_size_t)len) == (lfs_ssize_t)len) {
                out_rc = 0;
            }
        }
    } else if (sz == (lfs_soff_t)len) {
        /* Backwards-compatible read of pre-header raw 8 KiB file (upgrades to header on next save). */
        if (lfs_file_read(&g_lfs, &f, buf, (lfs_size_t)len) == (lfs_ssize_t)len) {
            out_rc = 0;
        }
    }
    if (lfs_file_close(&g_lfs, &f) != 0)
        return -1;
    return out_rc;
}

int fw_lfs_save_qmk_eeprom(uint8_t layout_id, const uint8_t *buf, size_t len) {
    if (!g_mounted || !buf || len == 0U || len > 65535U)
        return -1;
    char live[24], stage[28];
    qmk_ee_filenames(layout_id, live, stage);
    uint8_t hdr[FW_QMK_EE_HDR_LEN] = {
        k_qmk_ee_magic[0],
        k_qmk_ee_magic[1],
        k_qmk_ee_magic[2],
        k_qmk_ee_magic[3],
        (uint8_t)(FW_QMK_EE_SCHEMA_VER & 0xFFU),
        (uint8_t)(FW_QMK_EE_SCHEMA_VER >> 8),
        (uint8_t)(len & 0xFFU),
        (uint8_t)(len >> 8),
    };
    lfs_file_t f;
    if (lfs_file_open(&g_lfs, &f, stage, LFS_O_WRONLY | LFS_O_CREAT | LFS_O_TRUNC) != 0)
        return -1;
    lfs_ssize_t w1 = lfs_file_write(&g_lfs, &f, hdr, sizeof(hdr));
    lfs_ssize_t w2 = (w1 == (lfs_ssize_t)sizeof(hdr)) ? lfs_file_write(&g_lfs, &f, buf, (lfs_size_t)len) : -1;
    int close_rc = lfs_file_close(&g_lfs, &f);
    if (w1 == (lfs_ssize_t)sizeof(hdr) && w2 == (lfs_ssize_t)len && close_rc == 0 &&
        lfs_rename(&g_lfs, stage, live) == 0) {
        return 0;
    }
    (void)lfs_remove(&g_lfs, stage);
    return -1;
}

int fw_lfs_load_vial_def(uint8_t *buf, size_t cap, size_t *out_len) {
    if (!g_mounted || !buf || cap == 0U)
        return -1;
    size_t n = 0U;
    if (lfs_read_exact("vial_def.xz", buf, cap, &n) != 0 || n == 0U)
        return -1;
    if (out_len)
        *out_len = n;
    return 0;
}

int fw_lfs_save_vial_def(const uint8_t *buf, size_t len) {
    if (!g_mounted || !buf || len == 0U)
        return -1;
    if (lfs_write_file("vial_def.tmp", buf, len) == 0 && lfs_rename(&g_lfs, "vial_def.tmp", "vial_def.xz") == 0)
        return 0;
    (void)lfs_remove(&g_lfs, "vial_def.tmp");
    return -1;
}
