/* SPDX-License-Identifier: MIT */
/* RP2040 Work self-test for the dualcore carrier. The flash image is programmed
 * first; a RAM image of the same fixture runs the SWD and BOOTSEL checks. */
#include <string.h>

#include "hardware/clocks.h"
#include "hardware/dma.h"
#include "hardware/pio.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"

#include "pico_harness_spi.h"

#define DIRECT_MASK 0x1c03cc20u
#define BLOCK_SIZE 2048

/* All members are 32-bit; the host uses symbol addresses from this ELF. */
typedef struct {
    uint32_t magic, version, sequence, acknowledged;
    uint32_t command, argument, heartbeat, inputs;
    uint32_t uart_bytes, spi_blocks, spi_errors, spi_ready;
    uint32_t baud, direct_mask, spi_mode, status;
} work_test_t;

/* Separate diagnostics preserve the v1 mailbox layout used by worktest. */
typedef struct {
    uint32_t uart_done, uart_errors, signal_hz;
} work_speed_t;

static const uint pins[] = { 5, 10, 11, 14, 15, 16, 17, 26, 27, 28 };
static const uint16_t signal_code[] = { 0xe000, 0xe001 }; /* set pins, 0/1 */
static const struct pio_program signal_program = {
    .instructions = signal_code,
    .length = 2,
    .origin = -1,
};

volatile work_test_t work_test;
volatile uint32_t swd_scratch[1024];
static uint8_t spi_tx[BLOCK_SIZE], spi_rx[BLOCK_SIZE];
static bool spi_active, dynamic_gpio;
volatile work_speed_t work_speed;
static uint8_t uart_block[BLOCK_SIZE];
static int uart_rx_dma, uart_tx_dma;
static bool uart_dma_active, uart_dma_tx;
static uint signal_offset;
static int signal_pin = -1;

static void signal_stop(void)
{
    pio_sm_set_enabled(pio1, 0, false);
    if (signal_pin >= 0) {
        gpio_set_function(signal_pin, GPIO_FUNC_SIO);
        gpio_put(signal_pin, false);
        signal_pin = -1;
    }
}

static void signal_start(uint pin, uint32_t hz)
{
    signal_stop();
    dynamic_gpio = false;
    gpio_put_masked(DIRECT_MASK, 0);

    pio_sm_config c = pio_get_default_sm_config();
    sm_config_set_wrap(&c, signal_offset, signal_offset + 1);
    sm_config_set_set_pins(&c, pin, 1);
    uint32_t div256 = ((uint64_t)clock_get_hz(clk_sys) * 256) / (2 * hz);
    if (div256 < 256)
        div256 = 256;
    sm_config_set_clkdiv_int_frac(&c, div256 >> 8, div256 & 255);

    pio_sm_init(pio1, 0, signal_offset, &c);
    pio_sm_set_consecutive_pindirs(pio1, 0, pin, 1, true);
    pio_gpio_init(pio1, pin);
    signal_pin = pin;
    work_speed.signal_hz = ((uint64_t)clock_get_hz(clk_sys) * 128) / div256;
    pio_sm_set_enabled(pio1, 0, true);
}

static void uart_arm(void)
{
    dma_channel_abort(uart_rx_dma);
    dma_channel_abort(uart_tx_dma);
    while (uart_is_readable(uart0))
        (void)uart_getc(uart0);
    uart_get_hw(uart0)->rsr = 0;
    work_speed.uart_errors = 0;

    dma_channel_config c = dma_channel_get_default_config(uart_rx_dma);
    channel_config_set_transfer_data_size(&c, DMA_SIZE_8);
    channel_config_set_read_increment(&c, false);
    channel_config_set_write_increment(&c, true);
    channel_config_set_dreq(&c, DREQ_UART0_RX);
    dma_channel_configure(uart_rx_dma, &c, uart_block, &uart_get_hw(uart0)->dr, BLOCK_SIZE, true);
    uart_dma_tx = false;
    uart_dma_active = true;
}

static uint8_t pattern(uint32_t index)
{
    return (uint8_t)((index * 73u + 41u) ^ (index >> 3));
}

static void spi_arm(uint32_t mode)
{
    work_test.spi_ready = 0;
    for (uint i = 0; i < BLOCK_SIZE; ++i) {
        spi_tx[i] = pattern(i) ^ 0xa5;
        spi_rx[i] = 0;
    }
    spi_active = pico_harness_spi_arm(spi_tx, BLOCK_SIZE, mode);
    work_test.spi_mode = mode;
    work_test.spi_ready = spi_active;
}

__attribute__((noinline)) void work_tick(void)
{
    ++work_test.heartbeat;
    work_test.inputs = gpio_get_all();
}

int main(void)
{
    swd_scratch[0] = 0;
    work_test.version = 1;
    work_test.direct_mask = DIRECT_MASK;

    for (uint i = 0; i < count_of(pins); ++i) {
        gpio_init(pins[i]);
        gpio_disable_pulls(pins[i]);
        gpio_put(pins[i], false);
        gpio_set_dir(pins[i], GPIO_OUT);
    }

    work_test.baud = uart_init(uart0, 115200);
    gpio_set_function(12, GPIO_FUNC_UART);
    gpio_set_function(13, GPIO_FUNC_UART);

    pico_harness_spi_init();
    spi_arm(0);

    uart_rx_dma = dma_claim_unused_channel(true);
    uart_tx_dma = dma_claim_unused_channel(true);
    pio_sm_claim(pio1, 0);
    signal_offset = pio_add_program(pio1, &signal_program);

    work_test.magic = 0x56313254; /* mailbox signature */
    uint32_t last_gpio = time_us_32(), index = 0;

    while (true) {
        work_tick();

        if (work_test.sequence != work_test.acknowledged) {
            uint32_t sequence = work_test.sequence;
            work_test.status = 0;
            switch (work_test.command) {
            case 1: /* Static level for walking-one/zero wiring checks. */
                signal_stop();
                dynamic_gpio = false;
                gpio_put_masked(DIRECT_MASK, work_test.argument & DIRECT_MASK);
                break;
            case 2: /* Walking pulse, one pin per 100 us. */
                signal_stop();
                dynamic_gpio = true;
                break;
            case 3:
                dma_channel_abort(uart_rx_dma);
                dma_channel_abort(uart_tx_dma);
                uart_dma_active = false;
                work_test.baud = uart_set_baudrate(uart0, work_test.argument);
                break;
            case 6: /* A full-speed DMA receive, then DMA echo of 2048 bytes. */
                uart_arm();
                break;
            case 7:
                signal_stop();
                break;
            case 8: { /* Pin in bits 4:0, requested square wave Hz in 31:5. */
                uint pin = work_test.argument & 31;
                uint32_t hz = work_test.argument >> 5;
                if (!(DIRECT_MASK & (1u << pin)) || !hz || hz > clock_get_hz(clk_sys) / 2)
                    work_test.status = 1;
                else
                    signal_start(pin, hz);
                break;
            }
            case 4:
                spi_arm(work_test.argument & 3);
                break;
            case 5: /* Fill the SWD scratch area with a deterministic byte. */
                memset((void *)swd_scratch, work_test.argument & 0xff, sizeof(swd_scratch));
                break;
            default:
                work_test.status = 1;
                break;
            }
            work_test.acknowledged = sequence;
        }

        /* Complete a full DMA UART block before returning to byte echo. */
        if (uart_dma_active && !uart_dma_tx && !dma_channel_is_busy(uart_rx_dma)) {
            work_speed.uart_errors |= uart_get_hw(uart0)->rsr;
            dma_channel_config c = dma_channel_get_default_config(uart_tx_dma);
            channel_config_set_transfer_data_size(&c, DMA_SIZE_8);
            channel_config_set_read_increment(&c, true);
            channel_config_set_write_increment(&c, false);
            channel_config_set_dreq(&c, DREQ_UART0_TX);
            dma_channel_configure(uart_tx_dma, &c, &uart_get_hw(uart0)->dr, uart_block, BLOCK_SIZE,
                                  true);
            uart_dma_tx = true;
        }

        if (uart_dma_active && uart_dma_tx && !dma_channel_is_busy(uart_tx_dma) &&
            !(uart_get_hw(uart0)->fr & UART_UARTFR_BUSY_BITS)) {
            ++work_speed.uart_done;
            /* Remain in DMA mode until re-armed or baud is reconfigured. */
            uart_dma_tx = false;
            uart_dma_active = false;
        }

        while (!uart_dma_active && uart_is_readable(uart0) && uart_is_writable(uart0)) {
            uart_putc_raw(uart0, uart_getc(uart0));
            ++work_test.uart_bytes;
        }

        if (spi_active && pico_harness_spi_done()) {
            pico_harness_spi_read(spi_rx, BLOCK_SIZE);
            pico_harness_spi_abort();
            uint32_t errors = 0;
            for (uint i = 0; i < BLOCK_SIZE; ++i)
                errors += spi_rx[i] != pattern(i);
            work_test.spi_errors += errors;
            ++work_test.spi_blocks;
            work_test.spi_ready = 0;
            spi_active = false;
        }

        uint32_t now = time_us_32();
        if (dynamic_gpio && (uint32_t)(now - last_gpio) >= 100) {
            last_gpio = now;
            gpio_put_masked(DIRECT_MASK, 1u << pins[index++ % count_of(pins)]);
        }
    }
}
