/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef PDBG_LOGIC_ANALYZER_H
#define PDBG_LOGIC_ANALYZER_H

#include <stdbool.h>
#include <stdint.h>

#define LA_SAMPLES 32768u

/* flags: bit0=input synchronizer bypass, bit1=DMA observed RX stall,
 * bit2=trigger timeout, bit3=cancelled, bit4=running, bit5=pre-trigger ring.
 * Bits 16..31 give the retained pre-trigger sample count (when bit5 is set).
 * Trigger: 0=immediate,1=rising,2=falling. */
typedef struct {
    uint32_t sample_hz, elapsed_us, flags, count;
} la_result_t;

void la_init(void);
bool la_start(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin, uint16_t timeout_ms,
              bool bypass, const volatile uint32_t *epoch, uint32_t expected_epoch, la_result_t *result);
bool la_poll(void);
bool la_busy(void);
void la_cancel(void);
bool la_capture(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin, uint16_t timeout_ms,
                bool bypass, const volatile uint32_t *epoch, uint32_t expected_epoch,
                la_result_t *result);
uint32_t la_sample(uint32_t index);

#endif
