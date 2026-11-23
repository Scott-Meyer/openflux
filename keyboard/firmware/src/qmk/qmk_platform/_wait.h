#pragma once

#include <stdint.h>

void fw_qmk_wait_ms(uint32_t ms);

#define wait_ms(ms) fw_qmk_wait_ms((uint32_t)(ms))
#define wait_us(us) ((void)(us))
#define waitInputPinDelay() ((void)0)
