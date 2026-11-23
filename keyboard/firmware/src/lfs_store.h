#ifndef FW_LFS_STORE_H
#define FW_LFS_STORE_H

#include "fw_scanner.h"
#include "fw_refinement.h"

#include <stddef.h>
#include <stdint.h>

int fw_lfs_mount(void);
#ifdef HOST_TEST
/* Simulated flash for host tests. */
void fw_lfs_host_reset_flash(void);
/* Return a simulated flash I/O error after n successful program calls; negative disables. */
void fw_lfs_host_fail_program_after(int successful_writes);
int fw_lfs_host_format_and_mount(void);
/* Formats with 255 or 256 blocks, then mounts the way the firmware does. */
int fw_lfs_host_format_blocks_and_mount(uint32_t block_count);
int fw_lfs_host_unmount_and_remount(void);
#endif
void fw_lfs_load_boot_state(fw_runtime_t *rt);

int fw_lfs_save_keymap(const uint8_t *blob, size_t len);
int fw_lfs_save_profile_settings(const uint8_t *xfer, size_t len);
int fw_lfs_save_u32(const char *filename, uint32_t value);
int fw_lfs_save_rest_ref(const fw_runtime_t *rt);
int fw_lfs_load_rest_ref(fw_runtime_t *rt);

/* Per-layout QMK/Vial EEPROM and dynamic XZ vial.json storage (separate from factory calibration).
 * fw_lfs_load_qmk_eeprom returns: 0 = loaded OK, 1 = absent (LFS_ERR_NOENT), -1 = mount/IO/corrupt error. */
int fw_lfs_load_qmk_eeprom(uint8_t layout_id, uint8_t *buf, size_t len);
int fw_lfs_save_qmk_eeprom(uint8_t layout_id, const uint8_t *buf, size_t len);
int fw_lfs_load_vial_def(uint8_t *buf, size_t cap, size_t *out_len);
int fw_lfs_save_vial_def(const uint8_t *buf, size_t len);

/* Only INSTALLED yields a record suitable for the scanner. Read into scratch, not active model state. */
fw_refinement_state_t fw_lfs_read_refinement(uint8_t record[FW_REFINEMENT_RECORD_BYTES]);
/* Validates, writes and verifies staging before atomic replacement. Does not accept a path. */
int fw_lfs_install_refinement(const uint8_t *record, size_t len);

#endif /* FW_LFS_STORE_H */
