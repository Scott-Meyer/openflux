HOST_CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -Werror -std=c11 -fsanitize=address,undefined -fno-omit-frame-pointer
QMK_CFLAGS ?= -O2 -std=gnu11 -Wall -Wno-unused-parameter -Wno-unused-function -Wno-sign-compare -Wno-int-to-void-pointer-cast -fsanitize=address,undefined -fno-omit-frame-pointer
BUILD_DIR ?= build
# The firmware version has one source: KEYGLASS_FIRMWARE_VERSION in CMakeLists.txt.
VERSION ?= $(shell sed -n 's/^set(KEYGLASS_FIRMWARE_VERSION "\([^"]*\)").*/\1/p' CMakeLists.txt)

CORE_SRCS = src/fw_calibration.c src/fw_classic_row.c src/fw_classic_neighbors.c src/fw_classic_keys.c src/fw_depth.c src/fw_gamepad.c src/fw_hdlc.c src/fw_keyframe.c src/fw_lifecycle.c \
            src/fw_rapid.c src/fw_refine.c src/fw_refinement_record.c src/fw_rest.c src/fw_rest_runtime.c \
            src/fw_scanner.c src/fw_thermal.c src/fw_usb_cmd.c src/lfs_store.c
TEST_SRCS = tests/test_fw_core.c tests/test_fw_rapid.c tests/test_fw_refine.c tests/test_fw_refinement.c \
            tests/test_fw_rest.c tests/test_fw_thermal.c tests/test_fw_qmk.c
LFS_DIR ?= ../../third_party/littlefs
QMK_DIR ?= ../../third_party/vial-qmk

QMK_SRCS = \
	$(QMK_DIR)/quantum/action.c \
	$(QMK_DIR)/quantum/action_layer.c \
	$(QMK_DIR)/quantum/action_tapping.c \
	$(QMK_DIR)/quantum/action_util.c \
	$(QMK_DIR)/quantum/bitwise.c \
	$(QMK_DIR)/quantum/caps_word.c \
	$(QMK_DIR)/quantum/dynamic_keymap.c \
	$(QMK_DIR)/quantum/eeconfig.c \
	$(QMK_DIR)/quantum/keyboard.c \
	$(QMK_DIR)/quantum/keycode_config.c \
	$(QMK_DIR)/quantum/keymap_common.c \
	$(QMK_DIR)/quantum/keymap_introspection.c \
	$(QMK_DIR)/quantum/led.c \
	$(QMK_DIR)/quantum/logging/debug.c \
	$(QMK_DIR)/quantum/logging/print.c \
	$(QMK_DIR)/quantum/nvm/eeprom/nvm_dynamic_keymap.c \
	$(QMK_DIR)/quantum/nvm/eeprom/nvm_eeconfig.c \
	$(QMK_DIR)/quantum/nvm/eeprom/nvm_via.c \
	$(QMK_DIR)/quantum/process_keycode/process_auto_shift.c \
	$(QMK_DIR)/quantum/process_keycode/process_caps_word.c \
	$(QMK_DIR)/quantum/process_keycode/process_combo.c \
	$(QMK_DIR)/quantum/process_keycode/process_default_layer.c \
	$(QMK_DIR)/quantum/process_keycode/process_grave_esc.c \
	$(QMK_DIR)/quantum/process_keycode/process_key_override.c \
	$(QMK_DIR)/quantum/process_keycode/process_magic.c \
	$(QMK_DIR)/quantum/process_keycode/process_repeat_key.c \
	$(QMK_DIR)/quantum/process_keycode/process_tap_dance.c \
	$(QMK_DIR)/quantum/qmk_settings.c \
	$(QMK_DIR)/quantum/quantum.c \
	$(QMK_DIR)/quantum/raw_hid.c \
	$(QMK_DIR)/quantum/repeat_key.c \
	$(QMK_DIR)/quantum/send_string/send_string.c \
	$(QMK_DIR)/quantum/sync_timer.c \
	$(QMK_DIR)/quantum/via.c \
	$(QMK_DIR)/quantum/vial.c \
	$(QMK_DIR)/tmk_core/protocol/host.c \
	$(QMK_DIR)/tmk_core/protocol/report.c \
	$(QMK_DIR)/tmk_core/protocol/usb_device_state.c \
	$(QMK_DIR)/tmk_core/protocol/usb_util.c \
	$(QMK_DIR)/platforms/suspend.c \
	src/qmk/fw_qmk_platform.c \
	src/qmk/fw_qmk.c

QMK_INCLUDES = \
	-Isrc/qmk/qmk_platform -Isrc/qmk -Isrc \
	-I$(QMK_DIR) -I$(QMK_DIR)/quantum -I$(QMK_DIR)/quantum/keymap_extras \
	-I$(QMK_DIR)/quantum/sequencer -I$(QMK_DIR)/quantum/process_keycode \
	-I$(QMK_DIR)/quantum/send_string -I$(QMK_DIR)/quantum/logging \
	-I$(QMK_DIR)/quantum/nvm -I$(QMK_DIR)/quantum/nvm/eeprom \
	-I$(QMK_DIR)/tmk_core/protocol -I$(QMK_DIR)/platforms

QMK_DEFS = -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' \
	-DQMK_KEYBOARD_H='"keyglass_qmk.h"' -DKEYMAP_C='"fw_qmk_keymap.c"' \
	-include src/qmk/qmk_platform/config.h

QMK_OBJS = $(patsubst %.c,$(BUILD_DIR)/qmk/%.o,$(notdir $(QMK_SRCS)))

vpath %.c $(sort $(dir $(QMK_SRCS)))

$(BUILD_DIR)/qmk/%.o: %.c src/qmk/fw_qmk.h src/qmk/fw_qmk_keymap.c src/qmk/qmk_platform/config.h
	@mkdir -p $(BUILD_DIR)/qmk
	$(HOST_CC) $(QMK_CFLAGS) $(QMK_INCLUDES) $(QMK_DEFS) -c $< -o $@

test: $(TEST_SRCS) $(CORE_SRCS) $(QMK_OBJS)
	@mkdir -p $(BUILD_DIR)
	$(HOST_CC) $(CFLAGS) -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' -Isrc -I$(LFS_DIR) -c -Wno-unused-function $(LFS_DIR)/lfs.c -o $(BUILD_DIR)/lfs.o
	$(HOST_CC) $(CFLAGS) -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' -Isrc -I$(LFS_DIR) -c $(LFS_DIR)/lfs_util.c -o $(BUILD_DIR)/lfs_util.o
	$(HOST_CC) $(CFLAGS) -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' -Isrc -I$(LFS_DIR) -o $(BUILD_DIR)/test_fw_core $(TEST_SRCS) $(CORE_SRCS) $(BUILD_DIR)/lfs.o $(BUILD_DIR)/lfs_util.o $(QMK_OBJS)
	$(if $(filter /%,$(BUILD_DIR)),,./)$(BUILD_DIR)/test_fw_core

clean:
	rm -rf $(BUILD_DIR)

.PHONY: test clean
