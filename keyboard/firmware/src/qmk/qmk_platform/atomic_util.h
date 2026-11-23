#pragma once

/* Single-core scan loop on RP2040 / host test: no nested QMK interrupt re-entry. */
static inline uint8_t __fw_qmk_atomic_enter(void) { return 0; }
static inline void __fw_qmk_atomic_exit(const uint8_t *s) { (void)s; }

#define ATOMIC_BLOCK_RESTORESTATE \
    for (uint8_t __s __attribute__((__cleanup__(__fw_qmk_atomic_exit))) = __fw_qmk_atomic_enter(), __todo = 1; __todo; __todo = 0)
#define ATOMIC_BLOCK_FORCEON ATOMIC_BLOCK_RESTORESTATE
