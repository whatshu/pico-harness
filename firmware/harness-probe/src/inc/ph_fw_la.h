/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef PH_FW_LOGIC_ANALYZER_H
#define PH_FW_LOGIC_ANALYZER_H

#include <stdbool.h>
#include <stdint.h>

#define PH_FW_LA_SAMPLES 40960u /* 160 KiB of raw sample storage */

/* flags: bit0=input synchronizer bypass, bit1=DMA observed RX stall,
 * bit2=trigger timeout, bit3=cancelled, bit4=running, bit5=pre-trigger ring.
 * Bits 16..31 give the retained pre-trigger sample count (when bit5 is set).
 * Trigger: 0=immediate,1=rising,2=falling. */
typedef struct {
    uint32_t sample_hz, elapsed_us, flags, count;
} ph_fw_la_result_t;

void ph_fw_la_init(void);
bool ph_fw_la_start(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin, uint16_t timeout_ms,
              bool bypass, const volatile uint32_t *epoch, uint32_t expected_epoch, ph_fw_la_result_t *result);
/* Selected raw GPIO span is packed in RAM. Immediate mode packs multiple
 * samples per DMA word; edge mode uses byte/halfword DMA to retain exact history. */
uint32_t ph_fw_la_capacity(uint32_t raw_mask, uint8_t trigger);
bool ph_fw_la_start_selected(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin,
                            uint16_t timeout_ms, uint32_t raw_mask,
                            const volatile uint32_t *epoch, uint32_t expected_epoch,
                            ph_fw_la_result_t *result);
bool ph_fw_la_poll(void);
bool ph_fw_la_busy(void);
void ph_fw_la_cancel(void);
bool ph_fw_la_capture(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin, uint16_t timeout_ms,
                bool bypass, const volatile uint32_t *epoch, uint32_t expected_epoch,
                ph_fw_la_result_t *result);
uint32_t ph_fw_la_sample(uint32_t index);
uint8_t ph_fw_la_data_byte(uint32_t index);

#endif
