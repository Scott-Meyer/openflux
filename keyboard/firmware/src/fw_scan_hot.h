#ifndef FW_SCAN_HOT_H
#define FW_SCAN_HOT_H

/* The Pico SDK startup copies .time_critical.* from flash into main SRAM. Give
 * each kernel/table its own section so a build without refinement can discard
 * it. Host and simulator builds use the same C without any linker dependency.
 * Installed refinement coefficients live in fw_runtime_t, not in flash. */
#if defined(PICO_ON_DEVICE) && PICO_ON_DEVICE
#include "pico.h"
#define FW_SCAN_CODE(name)  __not_in_flash_func(name)
#define FW_SCAN_TABLE(name) __not_in_flash("scan.table." #name) name
#else
#define FW_SCAN_CODE(name)  name
#define FW_SCAN_TABLE(name) name
#endif

#endif
