/* SPDX-License-Identifier: MIT */
/*
 * ============================================================================
 * Target IO layer
 * ============================================================================
 *
 * Everything in the probe that physically touches the work-board.  Higher
 * layers (worker.c, control_thread, the CMSIS-DAP shim in swd.c) never reach
 * into pins or controller registers directly; they call the target_* API
 * declared in target.h.
 *
 * The GP1 (Debug core) exposes four IO "engines" to the rest of the firmware:
 *
 *   target_led_*       system-status LED: mode state + gpio redraw.  Pattern
 *                      selection is public (any module may request a mode);
 *                      control_thread redraws once per loop.
 *
 *   target_serial_*    the two serial faces of the probe:
 *                        - USB identity: iSerialNumber from the flash UUID;
 *                        - CDC-UART bridge: DMA-driven UART behind the virtual
 *                          COM port.  RX DMA runs continuously, TX DMA drains
 *                          bounded pieces of a software ring.  All bridge
 *                          calls run on core 1 except init before launch, and
 *                          no RTOS call, DMA IRQ or per-byte CPU work happens
 *                          on the transfer core.
 *
 *   target_swd_*       the PIO bit stream behind the CMSIS-DAP SWD transfer.
 *                      Instead of bitbashing, each bit sequence is handed to a
 *                      PIO state machine for asynchronous completion.
 *
 *   target_io_*        the target power path: 3.3 V gate and run/boot
 *                      controls, the Work voltage ADC, the INA237 power
 *                      monitor on I2C, and the DMA-driven SPI transport.
 *                      ph_fw_target_io_set_ready() is the single gate that decides
 *                      whether any of this may drive the Work side.
 *
 * The control THREAD of this split (power policy, readiness) lives in
 * target_control.c; it decides, this file executes.
 * ============================================================================
 */

#include <stdint.h>

#include "hardware/adc.h"
#include "hardware/clocks.h"
#include "hardware/dma.h"
#include "hardware/gpio.h"
#include "hardware/i2c.h"
#include "hardware/pio.h"
#include "hardware/spi.h"
#include "hardware/sync.h"
#include "hardware/uart.h"
#include "pico/critical_section.h"
#include "pico/stdlib.h"
#include "pico/unique_id.h"

#include "probe.pio.h"
#include "ph_fw_target.h"
#include "tusb.h"

#define RING_SIZE 4096u
#define RING_MASK (RING_SIZE - 1u)
/* Both RP2040 and RP2350 support an ordinary 28-bit transfer count. */
#define RX_TRANSFERS 0x0fffffffu
#define UART PROBE_UART0_INTERFACE

#define INA_ADDR 0x40
#define I2C_TIMEOUT_US 5000
/* Work pins that must never be driven while the Work rail is off. */
#define CROSS_MASK (PH_FW_GPIO_RAW_MASK | (1u << PROBE_PIN_SWCLK) | (1u << PROBE_PIN_SWDIO))

/* Debug pins for timing probes (opt-in; see CU_SELECT_DEBUG_PINS). */
enum dbg_pins {
    DBG_PIN_WRITE = 1,
    DBG_PIN_WRITE_WAIT = 2,
    DBG_PIN_READ = 4,
    DBG_PIN_PKT = 8,
};

/* States the PIO command FIFO understands (see probe.pio). */
typedef enum probe_pio_command {
    CMD_WRITE = 0,
    CMD_SKIP,
    CMD_TURNAROUND,
    CMD_READ
} probe_pio_command_t;

struct probe_ctx {
    uint offset;
    uint initted;
};

CU_REGISTER_DEBUG_PINS(probe_timing)

/* Module state, grouped by hardware engine. */
static enum ph_fw_led_mode led_mode = PH_FW_LED_OFF;

/* iSerialNumber is filled from the flash ID by ph_fw_target_serial_init(). */
char ph_fw_usb_serial[PICO_UNIQUE_BOARD_ID_SIZE_BYTES * 2 + 1];
static pico_unique_board_id_t uID;

/* RX DMA writes a 4 KiB ring. Indices are monotonic positions, masked only
 * when indexing the buffers; serial_rx_base anchors the DMA counter. */
static uint8_t serial_rx_data[RING_SIZE] __attribute__((aligned(RING_SIZE)));
static uint8_t serial_tx_data[RING_SIZE];
static uint32_t serial_rx_read, serial_rx_base, serial_rx_last;
static uint32_t serial_tx_read, serial_tx_write, serial_tx_pending;
static uint32_t serial_rx_overruns;
static int serial_rx_dma, serial_tx_dma;
static bool serial_enabled;
static spin_lock_t *serial_lock;

/* The PIO program offset is used to encode command branch addresses. */
static struct probe_ctx probe;

static critical_section_t io_lock;
static bool io_ready;
static bool target_ina_ready;
static uint16_t work_millivolts;
static int spi_rx_dma, spi_tx_dma;

/* ============================================================================
 * System-status LED
 * ============================================================================ */

void ph_fw_target_led_set_mode(enum ph_fw_led_mode mode)
{
    led_mode = mode;
}

enum ph_fw_led_mode ph_fw_target_led_get_mode(void)
{
    return led_mode;
}

#ifdef PROBE_USB_CONNECTED_LED

void ph_fw_target_led_init(void)
{
    gpio_init(PROBE_USB_CONNECTED_LED);
    gpio_set_dir(PROBE_USB_CONNECTED_LED, GPIO_OUT);
}

/*
 * Redraw the LED for the current mode.
 *
 * PH_FW_LED_OFF     → solid OFF (USB suspended / disconnected)
 * PH_FW_LED_IDLE    → 2 short 100 ms flashes per second (USB up, no target yet)
 * PH_FW_LED_TARGET  → steady 1 Hz blink (SWD target detected, monitoring)
 * PH_FW_LED_CAPTURE → fast 5 Hz blink (GPIO capture active)
 * PH_FW_LED_ERROR   → solid ON (persistent fault)
 */
void ph_fw_target_led_tick(void)
{
    TickType_t now = xTaskGetTickCount();
    bool on = false;

    switch (ph_fw_target_led_get_mode()) {
    case PH_FW_LED_OFF:
        break;
    case PH_FW_LED_IDLE:
    default: {
        /* 2 short pulses per second. */
        TickType_t p = (now / pdMS_TO_TICKS(100)) % 10;
        on = tud_ready() && (p == 0 || p == 2);
        break;
    }
    case PH_FW_LED_TARGET: {
        /* 1 Hz: 500 ms on, 500 ms off. */
        TickType_t p = (now / pdMS_TO_TICKS(100)) % 10;
        on = tud_ready() && (p < 5);
        break;
    }
    case PH_FW_LED_CAPTURE: {
        /* 5 Hz: 100 ms on, 100 ms off. */
        TickType_t p = (now / pdMS_TO_TICKS(100)) % 2;
        on = tud_ready() && (p == 0);
        break;
    }
    case PH_FW_LED_ERROR:
        on = true;
        break;
    }

    gpio_put(PROBE_USB_CONNECTED_LED, on);
}

#else
/* No status LED on this board; mode tracking above still works. */
void ph_fw_target_led_init(void)
{
}

void ph_fw_target_led_tick(void)
{
}
#endif

/* ============================================================================
 * Compact serial bridge (USB identity + DMA-driven UART)
 * ============================================================================ */

/*
 * RX engine: one always-running DMA ring reads the UART into serial_rx_data.
 * TX engine: the software ring is drained in bounded chunks by a second DMA
 * channel whenever it goes idle.  All of the *_nl() helpers below assume the
 * caller holds serial_lock; the public entry points acquire it.
 */

/* (Re)arm the RX DMA ring so its counter space starts at serial_rx_base. */
static void target_serial_rx_start(void)
{
    dma_channel_config c = dma_channel_get_default_config(serial_rx_dma);
    channel_config_set_transfer_data_size(&c, DMA_SIZE_8);
    channel_config_set_read_increment(&c, false);
    channel_config_set_write_increment(&c, true);
    channel_config_set_ring(&c, true, 12);
    channel_config_set_dreq(&c, uart_get_dreq(UART, false));
    dma_channel_configure(serial_rx_dma, &c, serial_rx_data + (serial_rx_base & RING_MASK),
                          &uart_get_hw(UART)->dr, RX_TRANSFERS, true);
}

/* Rederive how much data the DMA ring has produced and count overruns. */
static uint32_t target_serial_rx_update(void)
{
    uint32_t write =
        serial_rx_base + RX_TRANSFERS - dma_channel_hw_addr(serial_rx_dma)->transfer_count;

    __dmb(); /* DMA data is visible before the producer index is used. */
    if (write - serial_rx_read > RING_SIZE) {
        serial_rx_overruns += write - serial_rx_read - RING_SIZE;
        serial_rx_read = write - RING_SIZE;
    }

    /* RSR is sticky; count error observations as well as exact ring drops. */
    if (uart_get_hw(UART)->rsr) {
        ++serial_rx_overruns;
        uart_get_hw(UART)->rsr = 0;
    }

    serial_rx_last = write;
    return write;
}

/* Engine housekeeping without the lock:
 *  - gate behind Work power: abort everything while the rail is off,
 *  - arm the RX ring (and restart it when it wraps),
 *  - push the next TX chunk once the previous one drained.
 * Returns false when the UART bridge is disabled. */
static bool target_serial_poll_nl(void)
{
    if (!ph_fw_target_io_ready()) {
        if (serial_enabled) {
            dma_channel_abort(serial_rx_dma);
            dma_channel_abort(serial_tx_dma);
            serial_enabled = false;
            serial_tx_read = serial_tx_write = serial_tx_pending = 0;
            serial_rx_read = serial_rx_base = serial_rx_last = 0;
        }
        return false;
    }

    if (!serial_enabled) {
        while (uart_is_readable(UART))
            (void)uart_getc(UART);
        target_serial_rx_start();
        serial_enabled = true;
    }

    target_serial_rx_update();
    if (!dma_channel_is_busy(serial_rx_dma)) {
        serial_rx_base = serial_rx_last;
        target_serial_rx_start();
    }

    if (serial_tx_pending && !dma_channel_is_busy(serial_tx_dma)) {
        serial_tx_read += serial_tx_pending;
        serial_tx_pending = 0;
    }

    if (!serial_tx_pending && serial_tx_write != serial_tx_read) {
        serial_tx_pending =
            MIN(serial_tx_write - serial_tx_read, RING_SIZE - (serial_tx_read & RING_MASK));
        dma_channel_config c = dma_channel_get_default_config(serial_tx_dma);
        channel_config_set_transfer_data_size(&c, DMA_SIZE_8);
        channel_config_set_read_increment(&c, true);
        channel_config_set_write_increment(&c, false);
        channel_config_set_dreq(&c, uart_get_dreq(UART, true));
        dma_channel_configure(serial_tx_dma, &c, &uart_get_hw(UART)->dr,
                              serial_tx_data + (serial_tx_read & RING_MASK), serial_tx_pending,
                              true);
    }

    return false;
}

bool ph_fw_target_serial_poll(void)
{
    uint32_t irq = spin_lock_blocking(serial_lock);
    bool result = target_serial_poll_nl();
    spin_unlock(serial_lock, irq);
    return result;
}

bool ph_fw_target_serial_config(uint8_t channel, uint32_t baud, uint8_t bits, uint8_t stop,
                          uint8_t parity, uint32_t *actual)
{
    uint32_t irq = spin_lock_blocking(serial_lock);

    if (channel || baud < 300 || baud > clock_get_hz(clk_peri) / 16 || bits < 5 || bits > 8 ||
        (stop != 1 && stop != 2) || parity > 2) {
        spin_unlock(serial_lock, irq);
        return false;
    }

    dma_channel_abort(serial_rx_dma);
    dma_channel_abort(serial_tx_dma);

    /* Discard queued data on reconfiguration, matching the original API. */
    uart_set_fifo_enabled(UART, false);
    *actual = uart_set_baudrate(UART, baud);
    uart_set_format(UART, bits, stop,
                    parity == 1 ? UART_PARITY_ODD :
                    parity == 2 ? UART_PARITY_EVEN :
                                  UART_PARITY_NONE);
    uart_set_fifo_enabled(UART, true);
    while (uart_is_readable(UART))
        (void)uart_getc(UART);
    uart_get_hw(UART)->rsr = 0;
    serial_rx_read = serial_rx_base = serial_rx_last = 0;
    serial_tx_read = serial_tx_write = serial_tx_pending = serial_rx_overruns = 0;
    serial_enabled = false;
    target_serial_poll_nl();
    spin_unlock(serial_lock, irq);

    return true;
}

size_t ph_fw_target_serial_write(uint8_t channel, const uint8_t *data, size_t length)
{
    uint32_t irq = spin_lock_blocking(serial_lock);
    if (channel) {
        spin_unlock(serial_lock, irq);
        return 0;
    }
    target_serial_poll_nl();
    size_t count = MIN(length, RING_SIZE - (serial_tx_write - serial_tx_read));
    for (size_t i = 0; i < count; ++i)
        serial_tx_data[(serial_tx_write + i) & RING_MASK] = data[i];
    serial_tx_write += count;
    target_serial_poll_nl();
    spin_unlock(serial_lock, irq);
    return count;
}

size_t ph_fw_target_serial_read(uint8_t channel, uint8_t *data, size_t length)
{
    uint32_t irq = spin_lock_blocking(serial_lock);
    if (channel || !serial_enabled) {
        spin_unlock(serial_lock, irq);
        return 0;
    }
    uint32_t write = target_serial_rx_update();
    uint32_t start = serial_rx_read;
    size_t count = MIN(length, write - start);
    for (size_t i = 0; i < count; ++i)
        data[i] = serial_rx_data[(serial_rx_read + i) & RING_MASK];
    /* Detect a producer lap during the copy rather than returning corrupt data. */
    uint32_t after = target_serial_rx_update();
    if (after - start > RING_SIZE) {
        spin_unlock(serial_lock, irq);
        return 0;
    }
    serial_rx_read += count;
    spin_unlock(serial_lock, irq);
    return count;
}

bool ph_fw_target_serial_status(uint8_t channel, uint16_t *available, uint16_t *space, uint32_t *errors)
{
    uint32_t irq = spin_lock_blocking(serial_lock);
    if (channel) {
        spin_unlock(serial_lock, irq);
        return false;
    }
    target_serial_poll_nl();
    *available = serial_enabled ? (uint16_t)(serial_rx_last - serial_rx_read) : 0;
    *space = (uint16_t)(RING_SIZE - (serial_tx_write - serial_tx_read));
    *errors = serial_rx_overruns;
    spin_unlock(serial_lock, irq);
    return true;
}

/* USB identity + CDC-UART bridge bring-up; call once, before tusb_init(). */
void ph_fw_target_serial_init(void)
{
    pico_get_unique_board_id(&uID);

    for (int i = 0; i < PICO_UNIQUE_BOARD_ID_SIZE_BYTES * 2; i++) {
        /* Byte index inside the uid array */
        int bi = i / 2;
        /* Use high nibble first to keep memory order (just cosmetics) */
        uint8_t nibble = (uID.id[bi] >> 4) & 0x0F;
        uID.id[bi] <<= 4;
        /* Binary to hex digit */
        ph_fw_usb_serial[i] = nibble < 10 ? nibble + '0' : nibble + 'A' - 10;
    }

    /* UART pins and DMA are ready before the worker core starts polling. */
    ph_fw_target_gpio_function(PROBE_UART0_TX, GPIO_FUNC_UART);
    ph_fw_target_gpio_function(PROBE_UART0_RX, GPIO_FUNC_UART);
    gpio_disable_pulls(PROBE_UART0_TX);
    gpio_disable_pulls(PROBE_UART0_RX);
    uart_init(UART, PROBE_UART_BAUDRATE);
    uart_set_hw_flow(UART, false, false);
    serial_rx_dma = dma_claim_unused_channel(true);
    serial_tx_dma = dma_claim_unused_channel(true);
    serial_lock = spin_lock_instance(spin_lock_claim_unused(true));
}

/* ============================================================================
 * SWD PIO engine (bit stream behind the CMSIS-DAP transfer)
 * ============================================================================ */

/* The DAP layer drives SWCLK/SWDIO through the four primitives below; swd.c
 * turns CMSIS-DAP packets into these bit sequences. */

void ph_fw_target_swd_set_clk_freq(uint freq_khz)
{
    uint clk_sys_freq_khz = clock_get_hz(clk_sys) / 1000;

    ph_fw_probe_info("Set swclk freq %dKHz sysclk %dkHz\n", freq_khz, clk_sys_freq_khz);
    /* Round up (otherwise fast swclks get faster) */
    uint32_t divider = (((clk_sys_freq_khz + freq_khz - 1) / freq_khz) + 3) / 4;
    if (divider == 0)
        divider = 1;
    if (divider > 65535)
        divider = 65535;

    pio_sm_set_clkdiv_int_frac(pio0, PROBE_SM, divider, 0);
}

void ph_fw_target_swd_assert_reset(bool state)
{
    /* Q1 inverts the Debug gate level into active-low Work RUN. */
    gpio_put(PROBE_PIN_RESET, !state);
    gpio_set_dir(PROBE_PIN_RESET, GPIO_OUT);
}

int ph_fw_target_swd_reset_level(void)
{
    return !gpio_get(PROBE_PIN_RESET);
}

/* Pack one command word: bit count (1..256), direction, and the target
 * instruction address inside the running program. */
static inline uint32_t fmt_probe_command(uint bit_count, bool out_en, probe_pio_command_t cmd)
{
    uint cmd_addr = cmd == CMD_WRITE      ? probe.offset + probe_offset_write_cmd :
                    cmd == CMD_SKIP       ? probe.offset + probe_offset_get_next_cmd :
                    cmd == CMD_TURNAROUND ? probe.offset + probe_offset_turnaround_cmd :
                                            probe.offset + probe_offset_read_cmd;
    return ((bit_count - 1) & 0xff) | ((uint)out_en << 8) | (cmd_addr << 9);
}

void ph_fw_target_swd_write_bits(uint bit_count, uint32_t data_byte)
{
    DEBUG_PINS_SET(probe_timing, DBG_PIN_WRITE);
    pio_sm_put_blocking(pio0, PROBE_SM, fmt_probe_command(bit_count, true, CMD_WRITE));
    pio_sm_put_blocking(pio0, PROBE_SM, data_byte);
    ph_fw_probe_dump("Write %d bits 0x%x\n", bit_count, data_byte);
    /* Return immediately so we can cue up the next command whilst this one runs */
    DEBUG_PINS_CLR(probe_timing, DBG_PIN_WRITE);
}

void ph_fw_target_swd_hiz_clocks(uint bit_count)
{
    pio_sm_put_blocking(pio0, PROBE_SM, fmt_probe_command(bit_count, false, CMD_TURNAROUND));
    pio_sm_put_blocking(pio0, PROBE_SM, 0);
}

uint32_t ph_fw_target_swd_read_bits(uint bit_count)
{
    DEBUG_PINS_SET(probe_timing, DBG_PIN_READ);
    pio_sm_put_blocking(pio0, PROBE_SM, fmt_probe_command(bit_count, false, CMD_READ));
    uint32_t data = pio_sm_get_blocking(pio0, PROBE_SM);
    uint32_t data_shifted = data;
    if (bit_count < 32) {
        data_shifted = data >> (32 - bit_count);
    }

    ph_fw_probe_dump("Read %d bits 0x%x (shifted 0x%x)\n", bit_count, data, data_shifted);
    DEBUG_PINS_CLR(probe_timing, DBG_PIN_READ);
    return data_shifted;
}

/* Wait until every queued command has been consumed by the state machine. */
static void probe_wait_idle(void)
{
    pio0->fdebug = 1u << (PIO_FDEBUG_TXSTALL_LSB + PROBE_SM);
    while (!(pio0->fdebug & (1u << (PIO_FDEBUG_TXSTALL_LSB + PROBE_SM))))
        ;
}

void ph_fw_target_swd_read_mode(void)
{
    pio_sm_put_blocking(pio0, PROBE_SM, fmt_probe_command(0, false, CMD_SKIP));
    probe_wait_idle();
}

void ph_fw_target_swd_write_mode(void)
{
    pio_sm_put_blocking(pio0, PROBE_SM, fmt_probe_command(0, true, CMD_SKIP));
    probe_wait_idle();
}

static void probe_gpio_init(void)
{
    gpio_init(PROBE_PIN_RESET);
    gpio_disable_pulls(PROBE_PIN_RESET);
    gpio_put(PROBE_PIN_RESET, false);
    gpio_set_dir(PROBE_PIN_RESET, GPIO_OUT);
    /* Funcsel pins */
    ph_fw_target_gpio_function(PROBE_PIN_SWCLK, GPIO_FUNC_PIO0);
    ph_fw_target_gpio_function(PROBE_PIN_SWDIO, GPIO_FUNC_PIO0);
    /* Make sure SWDIO has a pullup on it. Idle state is high */
    gpio_disable_pulls(PROBE_PIN_SWCLK);
    gpio_disable_pulls(PROBE_PIN_SWDIO);
}

/* DAP interface says all pins have to be High-Z when disabled */
static void probe_gpio_deinit(void)
{
    gpio_put(PROBE_PIN_RESET, false);
    gpio_set_dir(PROBE_PIN_RESET, GPIO_OUT);
    gpio_set_dir(PROBE_PIN_SWCLK, GPIO_IN);
    gpio_set_dir(PROBE_PIN_SWDIO, GPIO_IN);
    ph_fw_target_gpio_function(PROBE_PIN_SWCLK, GPIO_FUNC_SIO);
    ph_fw_target_gpio_function(PROBE_PIN_SWDIO, GPIO_FUNC_SIO);
}

static void probe_sm_init(pio_sm_config *sm_config)
{
    /* Set SWCLK as a sideset pin */
    sm_config_set_sideset_pins(sm_config, PROBE_PIN_SWCLK);

    /* Set SWDIO offset */
    sm_config_set_out_pins(sm_config, PROBE_PIN_SWDIO, 1);
    sm_config_set_set_pins(sm_config, PROBE_PIN_SWDIO, 1);
    sm_config_set_in_pins(sm_config, PROBE_PIN_SWDIO);

    /* Set SWD and SWDIO pins as output to start. This will be set in the sm */
    pio_sm_set_consecutive_pindirs(pio0, PROBE_SM, PROBE_PIN_OFFSET, 2, true);

    /* shift output right, autopull off, autopull threshold */
    sm_config_set_out_shift(sm_config, true, false, 0);
    /* shift input right as swd data is lsb first, autopush off */
    sm_config_set_in_shift(sm_config, true, false, 0);
}

void ph_fw_target_swd_init(void)
{
    if (!probe.initted) {
        probe_gpio_init();
        uint offset = pio_add_program(pio0, &probe_program);
        probe.offset = offset;

        pio_sm_config sm_config = probe_program_get_default_config(offset);
        probe_sm_init(&sm_config);
        pio_sm_init(pio0, PROBE_SM, offset, &sm_config);

        /* Set up divisor */
        ph_fw_target_swd_set_clk_freq(1000);

        /* Jump SM to command dispatch routine, and enable it */
        pio_sm_exec(pio0, PROBE_SM, offset + probe_offset_get_next_cmd);
        pio_sm_set_enabled(pio0, PROBE_SM, 1);
        probe.initted = 1;
    }
}

void ph_fw_target_swd_deinit(void)
{
    if (probe.initted) {
        ph_fw_target_swd_read_mode();
        pio_sm_set_enabled(pio0, PROBE_SM, 0);
        pio_remove_program(pio0, &probe_program, probe.offset);

        ph_fw_target_swd_assert_reset(1); /* de-assert nRESET */
        probe_gpio_deinit();
        probe.initted = 0;
    }
}

/* ============================================================================
 * Target power / measurement / SPI transport
 * ============================================================================ */

static bool target_ina_read(uint8_t reg, uint32_t *value, size_t count)
{
    uint8_t bytes[3];
    if (i2c_write_timeout_us(PH_FW_INA_I2C, INA_ADDR, &reg, 1, true, I2C_TIMEOUT_US) != 1 ||
        i2c_read_timeout_us(PH_FW_INA_I2C, INA_ADDR, bytes, count, false, I2C_TIMEOUT_US) !=
            (int)count)
        return false;
    *value = 0;
    for (size_t i = 0; i < count; ++i)
        *value = (*value << 8) | bytes[i];
    return true;
}

static bool target_ina_write(uint8_t reg, uint16_t value)
{
    uint8_t bytes[] = { reg, value >> 8, value & 0xff };
    return i2c_write_timeout_us(PH_FW_INA_I2C, INA_ADDR, bytes, 3, false, I2C_TIMEOUT_US) == 3;
}

static bool target_ina_init(void)
{
    uint32_t manufacturer;
    /* INA237: TI ZHCSNF5A sections 7.6 and 8.1.2. 50 mOhm, 100 uA/LSB.
     * ADCRANGE=0: 5 uV/LSB; CAL = 819.2e6 * 0.0001 * 0.05 = 4096.
     * Continuous shunt/bus/temp, 1052 us each, 16 averages. */
    return target_ina_read(0x3e, &manufacturer, 2) && manufacturer == 0x5449 &&
           target_ina_write(0x00, 0x0000) && target_ina_write(0x02, 4096) &&
           target_ina_write(0x01, 0xfb6a);
}

/* Apply the control layer's readiness decision: only when the Work rail is
 * powered and healthy may this layer drive any Work pin. */
void ph_fw_target_io_set_ready(bool ready)
{
    critical_section_enter_blocking(&io_lock);
    io_ready = ready;
    for (uint pin = 0; pin < 30; ++pin) {
        if (CROSS_MASK & (1u << pin))
            gpio_set_oeover(pin, ready ? GPIO_OVERRIDE_NORMAL : GPIO_OVERRIDE_LOW);
    }
    critical_section_exit(&io_lock);
}

bool ph_fw_target_io_ready(void)
{
    critical_section_enter_blocking(&io_lock);
    bool ready = io_ready;
    critical_section_exit(&io_lock);
    return ready;
}

void ph_fw_target_gpio_function(uint pin, gpio_function_t function)
{
    /* gpio_set_function clears OEOVER. Change only FUNCSEL so even a DAP
     * connect racing POWER_SET cannot briefly drive an unpowered Work pin. */
    critical_section_enter_blocking(&io_lock);
    hw_write_masked(&io_bank0_hw->io[pin].ctrl,
                    (uint32_t)function << IO_BANK0_GPIO0_CTRL_FUNCSEL_LSB,
                    IO_BANK0_GPIO0_CTRL_FUNCSEL_BITS);
    critical_section_exit(&io_lock);
}

uint16_t ph_fw_target_work_mv(void)
{
    /* GET_STATUS runs on core 1 while core 0 polls; the SDK ADC engine is a
     * START_ONCE/READY pair with no cross-core locking, so serialize it or a
     * lost conversion could hang the interrupt-free worker core forever. */
    critical_section_enter_blocking(&io_lock);
    adc_select_input(PH_FW_WORK_ADC_CHANNEL);
    /* 10k/10k divider, nominal 3.3 V reference. */
    work_millivolts = (uint32_t)adc_read() * 6600u / 4096u;
    critical_section_exit(&io_lock);
    return work_millivolts;
}

bool ph_fw_target_spi_transfer(const uint8_t *tx, uint8_t *rx, uint16_t length, ph_fw_target_yield_fn yield)
{
    gpio_put(PH_FW_SPI_CS_PIN, false);
    while (spi_is_readable(PH_FW_SPI_INTERFACE))
        (void)spi_get_hw(PH_FW_SPI_INTERFACE)->dr;

    /* Arm RX before TX so the first byte cannot be lost. */
    dma_channel_config dc = dma_channel_get_default_config(spi_rx_dma);
    channel_config_set_transfer_data_size(&dc, DMA_SIZE_8);
    channel_config_set_read_increment(&dc, false);
    channel_config_set_write_increment(&dc, true);
    channel_config_set_dreq(&dc, spi_get_dreq(PH_FW_SPI_INTERFACE, false));
    dma_channel_configure(spi_rx_dma, &dc, rx, &spi_get_hw(PH_FW_SPI_INTERFACE)->dr, length, false);

    dc = dma_channel_get_default_config(spi_tx_dma);
    channel_config_set_transfer_data_size(&dc, DMA_SIZE_8);
    channel_config_set_read_increment(&dc, true);
    channel_config_set_write_increment(&dc, false);
    channel_config_set_dreq(&dc, spi_get_dreq(PH_FW_SPI_INTERFACE, true));
    dma_channel_configure(spi_tx_dma, &dc, &spi_get_hw(PH_FW_SPI_INTERFACE)->dr, tx, length, false);
    dma_start_channel_mask((1u << spi_rx_dma) | (1u << spi_tx_dma));

    uint32_t start = time_us_32();
    bool failed = false;
    while (dma_channel_is_busy(spi_rx_dma) || dma_channel_is_busy(spi_tx_dma) ||
           spi_is_busy(PH_FW_SPI_INTERFACE)) {
        if (yield() || time_us_32() - start > 5000000 || !ph_fw_target_io_ready()) {
            failed = true;
            break;
        }
    }

    dma_channel_abort(spi_rx_dma);
    dma_channel_abort(spi_tx_dma);
    gpio_put(PH_FW_SPI_CS_PIN, true);
    return !failed;
}

void ph_fw_target_io_init(void)
{
    critical_section_init(&io_lock);

    const uint controls[] = { PH_FW_WORK_ENABLE_PIN, PH_FW_WORK_RUN_PIN, PH_FW_WORK_BOOT_PIN };
    for (uint i = 0; i < count_of(controls); ++i) {
        gpio_init(controls[i]);
        gpio_disable_pulls(controls[i]);
        gpio_put(controls[i], false);
        gpio_set_dir(controls[i], GPIO_OUT);
    }

    /* Keep every cross-domain pad gated until ph_fw_target_control_poll() approves it. */
    for (uint pin = 0; pin < 30; ++pin) {
        if (CROSS_MASK & (1u << pin)) {
            gpio_init(pin);
            gpio_disable_pulls(pin);
            gpio_set_oeover(pin, GPIO_OVERRIDE_LOW);
        }
    }

    ph_fw_target_gpio_function(PH_FW_SPI_RX_PIN, GPIO_FUNC_SPI);
    ph_fw_target_gpio_function(PH_FW_SPI_SCK_PIN, GPIO_FUNC_SPI);
    ph_fw_target_gpio_function(PH_FW_SPI_TX_PIN, GPIO_FUNC_SPI);
    gpio_put(PH_FW_SPI_CS_PIN, true);
    gpio_set_dir(PH_FW_SPI_CS_PIN, GPIO_OUT);
    spi_init(PH_FW_SPI_INTERFACE, 1000000);
    spi_rx_dma = dma_claim_unused_channel(true);
    spi_tx_dma = dma_claim_unused_channel(true);
    spi_set_format(PH_FW_SPI_INTERFACE, 8, SPI_CPOL_0, SPI_CPHA_0, SPI_MSB_FIRST);
    io_ready = false;

    gpio_init(PH_FW_WORK_FAULT_PIN);
    gpio_disable_pulls(PH_FW_WORK_FAULT_PIN);
    gpio_init(PH_FW_INA_ALERT_PIN);
    gpio_disable_pulls(PH_FW_INA_ALERT_PIN);

    adc_init();
    adc_gpio_init(PH_FW_WORK_ADC_PIN);
    i2c_init(PH_FW_INA_I2C, 400000);
    gpio_set_function(PH_FW_INA_SDA_PIN, GPIO_FUNC_I2C);
    gpio_set_function(PH_FW_INA_SCL_PIN, GPIO_FUNC_I2C);
    gpio_disable_pulls(PH_FW_INA_SDA_PIN);
    gpio_disable_pulls(PH_FW_INA_SCL_PIN);
    target_ina_ready = target_ina_init();
}

bool ph_fw_target_power_measure(int32_t values[4])
{
    if (!target_ina_ready) {
        target_ina_ready = target_ina_init();
        /* Allow one complete averaged conversion before reporting values. */
        if (target_ina_ready)
            busy_wait_ms(55);
    }

    uint32_t shunt, bus, current, power;
    if (!target_ina_ready || !target_ina_read(0x04, &shunt, 2) || !target_ina_read(0x05, &bus, 2) ||
        !target_ina_read(0x07, &current, 2) || !target_ina_read(0x08, &power, 3)) {
        target_ina_ready = false;
        return false;
    }

    /* MEASURE v2: signed int32 values in uV, uV, uA, uW. */
    values[0] = (int16_t)shunt * 5;
    values[1] = bus * 3125;
    values[2] = (int16_t)current * 100;
    values[3] = power * 20; /* 24-bit POWER, 0.2 * CURRENT_LSB W/bit. */
    return true;
}
