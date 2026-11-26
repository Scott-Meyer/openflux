cmake_minimum_required(VERSION 3.13)

set(PICO_BOARD pico)
# The keyboard's RP2040 has 16 MiB of flash; the top 1 MiB (from 0x00F00000) holds the LittleFS
# calibration file system, which a check after linking keeps the image out of.
add_compile_definitions(PICO_FLASH_SIZE_BYTES=16777216)

if(DEFINED ENV{PICO_SDK_PATH})
    include($ENV{PICO_SDK_PATH}/external/pico_sdk_import.cmake)
else()
    message(FATAL_ERROR "PICO_SDK_PATH must be set for RP2040 firmware build")
endif()

set(OPENFLUX_FIRMWARE_VERSION "2.1.4") # x-release-please-version
string(REGEX MATCH "^[0-9]+(\\.[0-9]+)*" _fw_cmake_version "${OPENFLUX_FIRMWARE_VERSION}")
if("${_fw_cmake_version}" STREQUAL "")
    message(FATAL_ERROR "Invalid OPENFLUX_FIRMWARE_VERSION: ${OPENFLUX_FIRMWARE_VERSION}")
endif()
project(openflux_firmware VERSION ${_fw_cmake_version} LANGUAGES C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

pico_sdk_init()

set(LFS_DIR "$ENV{LITTLEFS_PATH}")
if(NOT LFS_DIR)
    set(LFS_DIR "/opt/littlefs")
endif()

set(QMK_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/vial-qmk")

add_library(openflux_qmk STATIC
    ${QMK_DIR}/quantum/action.c
    ${QMK_DIR}/quantum/action_layer.c
    ${QMK_DIR}/quantum/action_tapping.c
    ${QMK_DIR}/quantum/action_util.c
    ${QMK_DIR}/quantum/bitwise.c
    ${QMK_DIR}/quantum/caps_word.c
    ${QMK_DIR}/quantum/dynamic_keymap.c
    ${QMK_DIR}/quantum/eeconfig.c
    ${QMK_DIR}/quantum/keyboard.c
    ${QMK_DIR}/quantum/keycode_config.c
    ${QMK_DIR}/quantum/keymap_common.c
    ${QMK_DIR}/quantum/keymap_introspection.c
    ${QMK_DIR}/quantum/led.c
    ${QMK_DIR}/quantum/logging/debug.c
    ${QMK_DIR}/quantum/logging/print.c
    ${QMK_DIR}/quantum/nvm/eeprom/nvm_dynamic_keymap.c
    ${QMK_DIR}/quantum/nvm/eeprom/nvm_eeconfig.c
    ${QMK_DIR}/quantum/nvm/eeprom/nvm_via.c
    ${QMK_DIR}/quantum/process_keycode/process_auto_shift.c
    ${QMK_DIR}/quantum/process_keycode/process_caps_word.c
    ${QMK_DIR}/quantum/process_keycode/process_combo.c
    ${QMK_DIR}/quantum/process_keycode/process_default_layer.c
    ${QMK_DIR}/quantum/process_keycode/process_grave_esc.c
    ${QMK_DIR}/quantum/process_keycode/process_key_override.c
    ${QMK_DIR}/quantum/process_keycode/process_magic.c
    ${QMK_DIR}/quantum/process_keycode/process_repeat_key.c
    ${QMK_DIR}/quantum/process_keycode/process_tap_dance.c
    ${QMK_DIR}/quantum/qmk_settings.c
    ${QMK_DIR}/quantum/quantum.c
    ${QMK_DIR}/quantum/raw_hid.c
    ${QMK_DIR}/quantum/repeat_key.c
    ${QMK_DIR}/quantum/send_string/send_string.c
    ${QMK_DIR}/quantum/sync_timer.c
    ${QMK_DIR}/quantum/via.c
    ${QMK_DIR}/quantum/vial.c
    ${QMK_DIR}/tmk_core/protocol/host.c
    ${QMK_DIR}/tmk_core/protocol/report.c
    ${QMK_DIR}/tmk_core/protocol/usb_device_state.c
    ${QMK_DIR}/tmk_core/protocol/usb_util.c
    ${QMK_DIR}/platforms/suspend.c
    src/qmk/fw_qmk_platform.c
    src/qmk/fw_qmk.c
)

target_include_directories(openflux_qmk PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src/qmk/qmk_platform
    ${CMAKE_CURRENT_SOURCE_DIR}/src/qmk
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${QMK_DIR}
    ${QMK_DIR}/quantum
    ${QMK_DIR}/quantum/keymap_extras
    ${QMK_DIR}/quantum/sequencer
    ${QMK_DIR}/quantum/process_keycode
    ${QMK_DIR}/quantum/send_string
    ${QMK_DIR}/quantum/logging
    ${QMK_DIR}/quantum/nvm
    ${QMK_DIR}/quantum/nvm/eeprom
    ${QMK_DIR}/tmk_core/protocol
    ${QMK_DIR}/platforms
)

target_compile_definitions(openflux_qmk PRIVATE
    FW_PROJECT_VERSION="${OPENFLUX_FIRMWARE_VERSION}"
    QMK_KEYBOARD_H="openflux_qmk.h"
    KEYMAP_C="fw_qmk_keymap.c"
)

target_compile_options(openflux_qmk PRIVATE
    -std=gnu11
    -Wall
    -Wno-unused-parameter
    -Wno-unused-function
    -Wno-sign-compare
    -Wno-int-to-pointer-cast
    -include ${CMAKE_CURRENT_SOURCE_DIR}/src/qmk/qmk_platform/config.h
)

add_executable(openflux_firmware
    src/rp2040_main.c
    src/fw_calibration.c
    src/fw_classic_row.c
    src/fw_classic_neighbors.c
    src/fw_classic_keys.c
    src/fw_depth.c
    src/fw_gamepad.c
    src/fw_keyframe.c
    src/fw_lifecycle.c
    src/fw_refine.c
    src/fw_rest.c
    src/fw_rest_runtime.c
    src/fw_thermal.c
    src/fw_hdlc.c
    src/fw_scanner.c
    src/fw_rapid.c
    src/fw_refinement_record.c
    src/fw_usb_cmd.c
    src/lfs_store.c
    src/usb_descriptors.c
    ${LFS_DIR}/lfs.c
    ${LFS_DIR}/lfs_util.c
)

target_include_directories(openflux_firmware PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${LFS_DIR}
)

set(FW_VERSION "${OPENFLUX_FIRMWARE_VERSION}")
if(DEFINED OPENFLUX_VERSION AND NOT "${OPENFLUX_VERSION}" STREQUAL "")
    set(FW_VERSION "${OPENFLUX_VERSION}")
endif()

option(FW_ENABLE_PRESS_REFINEMENT "Run installed press-refinement data in the scan loop" ON)
target_compile_definitions(openflux_firmware PRIVATE
    FW_PROJECT_VERSION="${FW_VERSION}"
    # Reserve all of core 0's SCRATCH_Y bank. The MPU protects the low 32 bytes.
    PICO_STACK_SIZE=4096
    PICO_USE_STACK_GUARDS=1
    FW_ENABLE_PRESS_REFINEMENT=$<BOOL:${FW_ENABLE_PRESS_REFINEMENT}>
)

target_compile_options(openflux_firmware PRIVATE -Wall -Wextra -Werror)
set_source_files_properties(${LFS_DIR}/lfs.c ${LFS_DIR}/lfs_util.c PROPERTIES COMPILE_OPTIONS "-Wno-unused-function")

target_link_libraries(openflux_firmware
    openflux_qmk
    pico_stdlib
    hardware_adc
    hardware_flash
    hardware_gpio
    hardware_sync
    hardware_uart
    tinyusb_device
    tinyusb_board
)

pico_add_extra_outputs(openflux_firmware)

# The image must end before the calibration file system, which starts 15 MiB into flash
# (FW_LFS_OFFSET_BYTES in src/fw_protocol.h), at XIP address 0x10000000 + 0x00F00000.
add_custom_command(TARGET openflux_firmware POST_BUILD
    COMMAND ${CMAKE_COMMAND}
        -DNM=${CMAKE_NM}
        -DELF=$<TARGET_FILE:openflux_firmware>
        -DLIMIT=0x10F00000
        -P ${CMAKE_CURRENT_SOURCE_DIR}/cmake/check_flash_end.cmake
    VERBATIM
)
