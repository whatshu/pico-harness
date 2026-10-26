/* SPDX-License-Identifier: MIT */
#include "pico_harness_spi.h"
#include "hardware/dma.h"
#include "hardware/pio.h"
#include "pico/stdlib.h"
#include "spi_slave.pio.h"

static uint32_t tx_words[PICO_HARNESS_SPI_BLOCK];
static uint32_t rx_words[PICO_HARNESS_SPI_BLOCK];
static int tx_dma, rx_dma;
static uint sm, offset;
static bool initialized, active;

void pico_harness_spi_init(void)
{
    if (initialized)
        return;
    sm = pio_claim_unused_sm(pio0, true);
    offset = pio_add_program(pio0, &harness_spi_slave_program);
    tx_dma = dma_claim_unused_channel(true);
    rx_dma = dma_claim_unused_channel(true);
    for (uint pin = 18; pin <= 21; ++pin) {
        pio_gpio_init(pio0, pin);
        gpio_disable_pulls(pin);
    }
    pio_sm_set_consecutive_pindirs(pio0, sm, 19, 1, true);
    initialized = true;
}

void pico_harness_spi_abort(void)
{
    if (!initialized)
        return;
    pio_sm_set_enabled(pio0, sm, false);
    dma_channel_abort(tx_dma);
    dma_channel_abort(rx_dma);
    active = false;
}

bool pico_harness_spi_arm(const uint8_t *transmit, size_t size, uint8_t mode)
{
    if (!size || size > PICO_HARNESS_SPI_BLOCK || mode > 3)
        return false;
    pico_harness_spi_init();
    pico_harness_spi_abort();
    uint cpol = mode >> 1, cpha = mode & 1;
    pio0->instr_mem[offset + 2] = pio_encode_wait_gpio(cpha ? !cpol : cpol, 18);
    pio0->instr_mem[offset + 4] = pio_encode_wait_gpio(cpha ? cpol : !cpol, 18);
    pio_sm_config config = harness_spi_slave_program_get_default_config(offset);
    sm_config_set_in_pins(&config, 20);
    sm_config_set_out_pins(&config, 19, 1);
    sm_config_set_in_shift(&config, false, true, 8);
    sm_config_set_out_shift(&config, false, true, 8);
    pio_sm_init(pio0, sm, offset, &config);
    pio_sm_clear_fifos(pio0, sm);
    for (size_t i = 0; i < size; ++i) {
        tx_words[i] = (uint32_t)(transmit ? transmit[i] : 0) << 24;
        rx_words[i] = 0;
    }
    dma_channel_config dc = dma_channel_get_default_config(tx_dma);
    channel_config_set_transfer_data_size(&dc, DMA_SIZE_32);
    channel_config_set_read_increment(&dc, true);
    channel_config_set_write_increment(&dc, false);
    channel_config_set_dreq(&dc, pio_get_dreq(pio0, sm, true));
    dma_channel_configure(tx_dma, &dc, &pio0->txf[sm], tx_words, size, false);
    dc = dma_channel_get_default_config(rx_dma);
    channel_config_set_transfer_data_size(&dc, DMA_SIZE_32);
    channel_config_set_read_increment(&dc, false);
    channel_config_set_write_increment(&dc, true);
    channel_config_set_dreq(&dc, pio_get_dreq(pio0, sm, false));
    dma_channel_configure(rx_dma, &dc, rx_words, &pio0->rxf[sm], size, false);
    dma_start_channel_mask((1u << tx_dma) | (1u << rx_dma));
    pio_sm_set_enabled(pio0, sm, true);
    active = true;
    return true;
}

bool pico_harness_spi_done(void)
{
    return active && !dma_channel_is_busy(rx_dma);
}

void pico_harness_spi_read(uint8_t *receive, size_t size)
{
    for (size_t i = 0; i < size && i < PICO_HARNESS_SPI_BLOCK; ++i)
        receive[i] = (uint8_t)rx_words[i];
}
