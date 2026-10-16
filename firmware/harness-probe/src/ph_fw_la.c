/* SPDX-License-Identifier: GPL-3.0-only
 * Adapted 2026-09-21 from gusmanb/logicanalyzer's startCaptureSimple and
 * configureCaptureDMAs. See doc/probe-logic-analyzer.md for attribution and license.
 * PIO/DMA owns sampling timing; core 1 polls completion without any IRQ.
 */

#include "hardware/clocks.h"
#include "hardware/dma.h"
#include "hardware/pio.h"
#include "pico/stdlib.h"

#include "ph_fw_la.h"
#include "logic_analyzer.pio.h"
#include "ph_fw_target.h"
#include "ph_fw_util.h"

#define SM 0

static uint32_t capture_buffer[PH_FW_LA_SAMPLES];
static uint burst_offset, counted_offset;
static int capture_dma, rewind_dma;
static uint32_t ring_start;
static bool active;
static bool active_triggered;
static uint32_t active_count, active_start, active_deadline, saved_bypass, capture_offset;
static const volatile uint32_t *active_epoch;
static uint32_t active_expected_epoch;
static ph_fw_la_result_t *active_result;

void ph_fw_la_init(void)
{
    pio_sm_claim(pio1, SM);
    burst_offset = pio_add_program(pio1, &la_burst_program);
    counted_offset = pio_add_program(pio1, &la_counted_program);
    capture_dma = dma_claim_unused_channel(true);
    rewind_dma = dma_claim_unused_channel(true);
    ring_start = (uintptr_t)capture_buffer;

    for (uint pin = 0; pin < 30; ++pin)
        if (PH_FW_GPIO_RAW_MASK & (1u << pin))
            gpio_set_input_enabled(pin, true);
}

bool ph_fw_la_start(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin, uint16_t timeout_ms,
              bool bypass, const volatile uint32_t *epoch, uint32_t expected_epoch,
              ph_fw_la_result_t *result)
{
    uint cycles = trigger ? 4 : 1;
    uint32_t clock = clock_get_hz(clk_sys);

    if (active || !count || count > PH_FW_LA_SAMPLES || hz < 2500 || hz > clock / cycles || trigger > 2 ||
        (trigger && (count < 4 || raw_pin >= 30 || !(PH_FW_GPIO_RAW_MASK & (1u << raw_pin)) ||
                     !timeout_ms || timeout_ms > 10000)))
        return false;

    /* Round up divider: actual sampling frequency never exceeds the request. */
    uint32_t div256 = ((uint64_t)clock * 256 + (uint64_t)hz * cycles - 1) / ((uint64_t)hz * cycles);
    *result = (ph_fw_la_result_t){ .sample_hz = ((uint64_t)clock * 256) / (div256 * cycles),
                             .flags = bypass ? 1u : 0u };

    /* Reset the sampler before selecting the triggered or immediate program. */
    pio_sm_set_enabled(pio1, SM, false);
    pio_sm_clear_fifos(pio1, SM);
    pio_sm_restart(pio1, SM);

    pio_sm_config c = trigger ? la_counted_program_get_default_config(counted_offset) :
                                la_burst_program_get_default_config(burst_offset);
    sm_config_set_in_pins(&c, 0);
    sm_config_set_in_shift(&c, false, true, 32);

    /* Trigger needs two setup TX words, so use the regular FIFO then.
     * Burst mode joins both FIFOs to tolerate DMA arbitration latency. */
    if (!trigger)
        sm_config_set_fifo_join(&c, PIO_FIFO_JOIN_RX);
    sm_config_set_clkdiv_int_frac(&c, div256 >> 8, div256 & 255);

    uint offset = trigger ? counted_offset : burst_offset;
    uint32_t history = trigger ? MAX(1u, count / 4) : 0;
    if (trigger) {
        sm_config_set_jmp_pin(&c, raw_pin);
        /* Only patch branch destinations: sampling cadence stays identical. */
        pio1->instr_mem[counted_offset + la_counted_arm_low] =
            pio_encode_jmp(counted_offset + (trigger == 1 ? la_counted_edge : la_counted_arm));
        pio1->instr_mem[counted_offset + la_counted_arm_high] =
            pio_encode_jmp(counted_offset + (trigger == 1 ? la_counted_arm : la_counted_edge));
        pio1->instr_mem[counted_offset + la_counted_edge_low] =
            pio_encode_jmp(counted_offset + (trigger == 1 ? la_counted_edge : la_counted_sample));
        pio1->instr_mem[counted_offset + la_counted_edge_high] =
            pio_encode_jmp(counted_offset + (trigger == 1 ? la_counted_sample : la_counted_edge));
    }
    pio_sm_init(pio1, SM, offset, &c);
    if (trigger) {
        pio_sm_put(pio1, SM, count - history - 1);
        pio_sm_put(pio1, SM, history - 1);
        result->flags |= 32u | (history << 16);
    }

    /* Save the shared bypass register so this capture leaves other pins intact. */
    saved_bypass = pio1->input_sync_bypass;
    hw_write_masked(&pio1->input_sync_bypass, bypass ? PH_FW_GPIO_RAW_MASK : 0, PH_FW_GPIO_RAW_MASK);
    pio_interrupt_clear(pio1, 0);
    pio1->fdebug = 1u << (PIO_FDEBUG_RXSTALL_LSB + SM);

    dma_channel_config dc = dma_channel_get_default_config(capture_dma);
    channel_config_set_transfer_data_size(&dc, DMA_SIZE_32);
    channel_config_set_read_increment(&dc, false);
    channel_config_set_write_increment(&dc, true);
    channel_config_set_high_priority(&dc, true);
    channel_config_set_dreq(&dc, pio_get_dreq(pio1, SM, false));
    dma_hw->intr = 1u << capture_dma;
    if (trigger) {
        /* A one-word control DMA rewinds the write address and retriggers the
         * sampler DMA. This supports the full 128 KiB buffer without the
         * RP2040's 32 KiB hardware address-ring limit or any CPU copying. */
        channel_config_set_chain_to(&dc, rewind_dma);
        dma_channel_config rc = dma_channel_get_default_config(rewind_dma);
        channel_config_set_transfer_data_size(&rc, DMA_SIZE_32);
        channel_config_set_read_increment(&rc, false);
        channel_config_set_write_increment(&rc, false);
        channel_config_set_high_priority(&rc, true);
        dma_channel_configure(rewind_dma, &rc,
                              &dma_channel_hw_addr(capture_dma)->al2_write_addr_trig,
                              &ring_start, 1, false);
    }
    dma_channel_configure(capture_dma, &dc, capture_buffer, &pio1->rxf[SM], count, true);

    active_count = count;
    active_triggered = trigger != 0;
    capture_offset = 0;
    active_start = time_us_32();
    active_deadline = (uint32_t)(((uint64_t)count * 1000000) / result->sample_hz) +
                      (trigger ? timeout_ms * 1000u : 0) + 100000u;
    active_epoch = epoch;
    active_expected_epoch = expected_epoch;
    active_result = result;
    result->flags |= 16u;
    active = true;
    pio_sm_set_enabled(pio1, SM, true);
    return true;
}

bool ph_fw_la_poll(void)
{
    if (!active)
        return true;
    ph_fw_la_result_t *result = active_result;
    uint32_t remaining = dma_channel_hw_addr(capture_dma)->transfer_count;
    uint32_t elapsed = time_us_32() - active_start;
    if (remaining > 8 && (pio1->fdebug & (1u << (PIO_FDEBUG_RXSTALL_LSB + SM))))
        result->flags |= 2;
    bool cancelled = *active_epoch != active_expected_epoch;
    bool timeout = elapsed > active_deadline;
    bool finished = active_triggered ? pio_interrupt_get(pio1, 0) :
                                     !dma_channel_is_busy(capture_dma);
    if (!finished && !cancelled && !timeout)
        return false;
    if (cancelled)
        result->flags |= 8;
    if (timeout && !finished)
        result->flags |= 4;
    pio_sm_set_enabled(pio1, SM, false);
    if (active_triggered) {
        /* Drain the last FIFO words before freezing the ring. No more samples
         * can arrive after the PIO completion IRQ / state-machine disable. */
        uint32_t drain_start = time_us_32();
        while (!pio_sm_is_rx_fifo_empty(pio1, SM) && time_us_32() - drain_start < 100)
            tight_loop_contents();
        hw_clear_bits(&dma_channel_hw_addr(rewind_dma)->ctrl_trig, DMA_CH0_CTRL_TRIG_EN_BITS);
        hw_clear_bits(&dma_channel_hw_addr(capture_dma)->ctrl_trig, DMA_CH0_CTRL_TRIG_EN_BITS);
        dma_channel_abort(rewind_dma);
        uint32_t position = (dma_channel_hw_addr(capture_dma)->write_addr - ring_start) / 4;
        bool wrapped = (dma_hw->intr & (1u << capture_dma)) != 0;
        result->count = wrapped ? active_count : MIN(position, active_count);
        capture_offset = wrapped ? position % active_count : 0;
    }
    /* DMA can finish between the initial transfer-count read and busy check.
     * Read it again after stopping the sampler so the final count is current. */
    remaining = dma_channel_hw_addr(capture_dma)->transfer_count;
    if (!active_triggered)
        result->count = active_count - remaining;
    /* Completion polling can be delayed by a concurrent SPI job. The PIO
     * divider, rather than host polling latency, defines the sample period. */
    result->elapsed_us = !finished ? MAX(1u, elapsed) :
        MAX(1u, (uint32_t)(((uint64_t)result->count * 1000000) / result->sample_hz));
    dma_channel_abort(capture_dma);
    pio1->input_sync_bypass = saved_bypass;
    result->flags &= ~16u;
    active = false;
    return true;
}

bool ph_fw_la_busy(void)
{
    return active;
}
void ph_fw_la_cancel(void)
{
    if (active) {
        active_expected_epoch = ~*active_epoch;
        ph_fw_la_poll();
    }
}

bool ph_fw_la_capture(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t raw_pin, uint16_t timeout_ms,
                bool bypass, const volatile uint32_t *epoch, uint32_t expected_epoch,
                ph_fw_la_result_t *result)
{
    if (!ph_fw_la_start(count, hz, trigger, raw_pin, timeout_ms, bypass, epoch, expected_epoch, result))
        return false;
    while (!ph_fw_la_poll())
        ph_fw_target_serial_poll();
    return true;
}

uint32_t ph_fw_la_sample(uint32_t index)
{
    return ph_fw_normalize_gpio(capture_buffer[(capture_offset + index) % active_count]);
}
