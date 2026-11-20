HOST_CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -Werror -std=c11 -fsanitize=address,undefined -fno-omit-frame-pointer
BUILD_DIR ?= build
# The firmware version has one source: OPENFLUX_FIRMWARE_VERSION in CMakeLists.txt.
VERSION ?= $(shell sed -n 's/^set(OPENFLUX_FIRMWARE_VERSION "\([^"]*\)").*/\1/p' CMakeLists.txt)

CORE_SRCS = src/fw_calibration.c src/fw_classic_row.c src/fw_classic_neighbors.c src/fw_classic_keys.c src/fw_depth.c src/fw_hdlc.c src/fw_keyframe.c src/fw_lifecycle.c \
            src/fw_rapid.c src/fw_refine.c src/fw_refinement_record.c src/fw_rest.c src/fw_rest_runtime.c \
            src/fw_scanner.c src/fw_thermal.c src/fw_usb_cmd.c src/lfs_store.c
TEST_SRCS = tests/test_fw_core.c tests/test_fw_rapid.c tests/test_fw_refine.c tests/test_fw_refinement.c \
            tests/test_fw_rest.c tests/test_fw_thermal.c
LFS_DIR ?= ../../third_party/littlefs

test: $(TEST_SRCS) $(CORE_SRCS)
	@mkdir -p $(BUILD_DIR)
	$(HOST_CC) $(CFLAGS) -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' -Isrc -I$(LFS_DIR) -c -Wno-unused-function $(LFS_DIR)/lfs.c -o $(BUILD_DIR)/lfs.o
	$(HOST_CC) $(CFLAGS) -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' -Isrc -I$(LFS_DIR) -c $(LFS_DIR)/lfs_util.c -o $(BUILD_DIR)/lfs_util.o
	$(HOST_CC) $(CFLAGS) -DHOST_TEST -DFW_PROJECT_VERSION='"$(VERSION)"' -Isrc -I$(LFS_DIR) -o $(BUILD_DIR)/test_fw_core $(TEST_SRCS) $(CORE_SRCS) $(BUILD_DIR)/lfs.o $(BUILD_DIR)/lfs_util.o
	./$(BUILD_DIR)/test_fw_core

clean:
	rm -rf $(BUILD_DIR)

.PHONY: test clean
