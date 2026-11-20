# Fails the build when the firmware image reaches the flash that holds the calibration file
# system. Run with -DNM=<nm> -DELF=<firmware.elf> -DLIMIT=<first reserved XIP address>.
execute_process(
    COMMAND "${NM}" "${ELF}"
    OUTPUT_VARIABLE symbols
    RESULT_VARIABLE nm_result
)
if(NOT nm_result EQUAL 0)
    message(FATAL_ERROR "${NM} ${ELF} failed")
endif()
string(REGEX MATCH "([0-9a-fA-F]+) [A-Za-z] __flash_binary_end" found "${symbols}")
if(NOT found)
    message(FATAL_ERROR "__flash_binary_end not found in ${ELF}")
endif()
math(EXPR image_end_dec "0x${CMAKE_MATCH_1}")
math(EXPR limit_dec "${LIMIT}")
math(EXPR image_end "${image_end_dec}" OUTPUT_FORMAT HEXADECIMAL)
math(EXPR limit "${limit_dec}" OUTPUT_FORMAT HEXADECIMAL)
if(image_end_dec GREATER limit_dec)
    message(FATAL_ERROR "Firmware image ends at ${image_end}, past ${limit} where the calibration file system starts")
endif()
message(STATUS "Firmware image ends at ${image_end}, before the calibration file system at ${limit}")
