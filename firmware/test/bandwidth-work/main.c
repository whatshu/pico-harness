/* SPDX-License-Identifier: MIT */
/* RAM-only receiver for dualcore wiring. Flash is untouched; reset restores it.
 * SWD reads occur outside throughput timing. Never arms through SWD per block. */
#include "hardware/clocks.h"
#include "hardware/dma.h"
#include "hardware/pio.h"
#include "pico/stdlib.h"

#include "spi_rx.pio.h"

#define MAX_BYTES 4095

typedef struct {
    uint32_t magic, sequence, acknowledged, length;
    uint32_t blocks, bytes, errors, ready;
    uint32_t first_block, first_index, first_expected, first_actual;
    uint32_t system_hz, signal_hz, heartbeat, status;
} bandwidth_t;

static const uint16_t signal_code[] = { 0xe000, 0xe001 };
static const struct pio_program signal_program = {
    .instructions = signal_code,
    .length = 2,
    .origin = -1,
};

volatile bandwidth_t bandwidth;
static uint32_t received[MAX_BYTES];
static int rx_dma;
static uint rx_offset;

static uint8_t pattern(uint32_t i, uint32_t block)
{
    return (uint8_t)(((i * 73u + 41u) ^ (i >> 3)) + block * 17u);
}

static void arm(void)
{
    pio_sm_set_enabled(pio0, 0, false);
    dma_channel_abort(rx_dma);

    pio_sm_config c = benchmark_spi_rx_program_get_default_config(rx_offset);
    sm_config_set_in_pins(&c, 20);
    sm_config_set_in_shift(&c, false, true, 8);
    sm_config_set_fifo_join(&c, PIO_FIFO_JOIN_RX);
    pio_sm_init(pio0, 0, rx_offset, &c);

    dma_channel_config d = dma_channel_get_default_config(rx_dma);
    channel_config_set_transfer_data_size(&d, DMA_SIZE_32);
    channel_config_set_read_increment(&d, false);
    channel_config_set_write_increment(&d, true);
    channel_config_set_dreq(&d, pio_get_dreq(pio0, 0, false));
    dma_channel_configure(rx_dma, &d, received, &pio0->rxf[0], bandwidth.length, true);
    pio_sm_set_enabled(pio0, 0, true);

    bandwidth.ready = 1;
}

int main(void)
{
    for (uint pin = 18; pin <= 21; ++pin) {
        gpio_init(pin);
        gpio_disable_pulls(pin);
    }

    /* A defined MISO level; downlink test does not require MISO data. */
    gpio_pull_down(19);
    pio_gpio_init(pio0, 18);
    pio_gpio_init(pio0, 20);
    pio_gpio_init(pio0, 21);
    pio_sm_claim(pio0, 0);
    rx_offset = pio_add_program(pio0, &benchmark_spi_rx_program);
    rx_dma = dma_claim_unused_channel(true);

    /* Known activity on the carrier's ordinary Work GP5 observation path. */
    uint offset = pio_add_program(pio1, &signal_program);
    pio_sm_claim(pio1, 0);
    pio_sm_config c = pio_get_default_sm_config();
    sm_config_set_wrap(&c, offset, offset + 1);
    sm_config_set_set_pins(&c, 5, 1);
    sm_config_set_clkdiv(&c, (float)clock_get_hz(clk_sys) / 200000.0f);
    pio_gpio_init(pio1, 5);
    pio_sm_set_consecutive_pindirs(pio1, 0, 5, 1, true);
    pio_sm_init(pio1, 0, offset, &c);
    pio_sm_set_enabled(pio1, 0, true);
    bandwidth.system_hz = clock_get_hz(clk_sys);
    bandwidth.signal_hz = 100000;
    bandwidth.magic = 0x42575431;

    while (true) {
        ++bandwidth.heartbeat;

        if (bandwidth.sequence != bandwidth.acknowledged) {
            pio_sm_set_enabled(pio0, 0, false);
            dma_channel_abort(rx_dma);
            bandwidth.ready = 0;
            bandwidth.blocks = bandwidth.bytes = bandwidth.errors = 0;
            bandwidth.first_block = bandwidth.first_index = 0;
            bandwidth.first_expected = bandwidth.first_actual = 0;
            bandwidth.status = bandwidth.length > MAX_BYTES;
            if (bandwidth.length && !bandwidth.status)
                arm();
            bandwidth.acknowledged = bandwidth.sequence;
        }

        if (bandwidth.ready && !dma_channel_is_busy(rx_dma)) {
            bandwidth.ready = 0;
            for (uint i = 0; i < bandwidth.length; ++i) {
                uint8_t expected = pattern(i, bandwidth.blocks);
                if (received[i] != expected) {
                    if (!bandwidth.errors) {
                        bandwidth.first_block = bandwidth.blocks;
                        bandwidth.first_index = i;
                        bandwidth.first_expected = expected;
                        bandwidth.first_actual = received[i];
                    }
                    ++bandwidth.errors;
                }
            }
            bandwidth.bytes += bandwidth.length;
            ++bandwidth.blocks;

            /* Ensure a still-low CS cannot be mistaken for the next block. */
            while (!gpio_get(21))
                tight_loop_contents();
            arm();
        }
    }
}
