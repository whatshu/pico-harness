/* SPDX-License-Identifier: MIT */
/*
 * ============================================================================
 * Target API: control policy + the IO layer that executes it
 * ============================================================================
 *
 * The Debug core owns two layers facing the work-board:
 *
 *   target_control.c  the CONTROL layer.  Power-policy decisions only: whether
 *                     the Work rail is up and healthy enough to drive, and the
 *                     enable/run/boot sequencing.  It never touches controller
 *                     registers directly; it reads (target_work_mv(),
 *                     target_io_ready(), gpio) and applies (target_power_set,
 *                     target_io_set_ready()).
 *
 *   target_io.c       the IO layer.  Every register, PIO, DMA and gpio
 *                     operation, split into four engines (serial, swd, led,
 *                     power/measurement/SPI).  See the file header there.
 *
 * Both files share this header, so any module talks to the target through the
 * target_* API and never reaches into pins itself.
 */

#ifndef PDBG_TARGET_H
#define PDBG_TARGET_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "pico/stdlib.h"
#include "FreeRTOS.h"
#include "task.h"
#include "pico_dualcore_config.h"

/* ------------------------------------------------------------------------
 * Debug helpers.  Dummy expansion when not enabled; probe_info / probe_debug
 * / probe_dump differ only in intent, not behaviour.
 * ------------------------------------------------------------------------ */

#if false
#define probe_info(format, args...) \
    do {                            \
        vTaskSuspendAll();          \
        printf(format, ##args);     \
        xTaskResumeAll();           \
    } while (0)
#else
#define probe_info(format, ...) ((void)0)
#endif

#if false
#define probe_debug(format, args...) \
    do {                             \
        vTaskSuspendAll();           \
        printf(format, ##args);      \
        xTaskResumeAll();            \
    } while (0)
#else
#define probe_debug(format, ...) ((void)0)
#endif

#if false
#define probe_dump(format, args...) \
    do {                            \
        vTaskSuspendAll();          \
        printf(format, ##args);     \
        xTaskResumeAll();           \
    } while (0)
#else
#define probe_dump(format, ...) ((void)0)
#endif

/* Returns true to abort an in-progress SPI transfer after caller housekeeping. */
typedef bool (*target_yield_fn)(void);

enum target_led_mode {
    TARGET_LED_OFF, /* USB suspended or not configured */
    TARGET_LED_IDLE, /* USB ready, no SWD target seen yet */
    TARGET_LED_TARGET, /* SWD target detected, idle / monitoring */
    TARGET_LED_CAPTURE, /* GPIO capture running */
    TARGET_LED_ERROR, /* persistent fault (power bad, watchdog, etc.) */
};

/* ------------------------------------------------------------------------
 * Control layer (target_control.c)
 * ------------------------------------------------------------------------ */

/* Poll the Work rail health and drive the IO readiness gate.  Called from
 * core 0's board-monitoring loop every ~5 ms. */
void target_control_poll(void);

/* Power the Work rail up or down; sequences the enable/run/boot controls. */
void target_power_set(bool enabled);

/* ------------------------------------------------------------------------
 * IO layer: power / measurement / SPI (target_io.c)
 * ------------------------------------------------------------------------ */

/* Bring up every controller owned by the IO layer.  Call once, first, on
 * core 0 before any task starts. */
void target_io_init(void);

/* Is the Work rail powered and healthy, i.e. may any Work pin be driven? */
bool target_io_ready(void);

/* Apply the control layer's readiness decision: gate the cross-domain drive. */
void target_io_set_ready(bool ready);

/* Latest Work rail voltage in millivolts (converts the ADC sample). */
uint16_t target_work_mv(void);

/* Change the function selection of a pad without touching its output enable,
 * so it can never briefly drive an unpowered Work pin. */
void target_gpio_function(uint pin, gpio_function_t function);

/* DMA-driven SPI transaction.  Runs on the Work side; yields while busy. */
bool target_spi_transfer(const uint8_t *tx, uint8_t *rx, uint16_t length, target_yield_fn yield);

/* INA237 power monitor readout: shunt/bus voltage, current and power (uV/uA/uW). */
bool target_power_measure(int32_t values[4]);

/* ------------------------------------------------------------------------
 * IO layer: compact serial bridge (DMA UART; USB identity in usb_serial)
 * ------------------------------------------------------------------------ */

/* iSerialNumber string in the USB device descriptor; NUL terminated after
 * target_serial_init(). */
extern char usb_serial[];

void target_serial_init(void);
bool target_serial_poll(void);
bool target_serial_config(uint8_t channel, uint32_t baud, uint8_t data_bits, uint8_t stop_bits,
                          uint8_t parity, uint32_t *actual_baud);
size_t target_serial_write(uint8_t channel, const uint8_t *data, size_t length);
size_t target_serial_read(uint8_t channel, uint8_t *data, size_t length);
bool target_serial_status(uint8_t channel, uint16_t *rx_available, uint16_t *tx_space,
                          uint32_t *rx_overruns);

/* ------------------------------------------------------------------------
 * IO layer: system-status LED
 * ------------------------------------------------------------------------ */

/* Any module may pick the state the LED should convey; control_thread calls
 * target_led_tick() once per loop to redraw the pattern.  The engine owns
 * the LED gpio, so callers never reach into the pins directly. */
void target_led_init(void);
void target_led_tick(void);
void target_led_set_mode(enum target_led_mode mode);
enum target_led_mode target_led_get_mode(void);

/* ------------------------------------------------------------------------
 * IO layer: SWD PIO engine (bit stream behind the CMSIS-DAP transfer)
 * ------------------------------------------------------------------------ */

/* Load the PIO program and bring the state machine up (idempotent). */
void target_swd_init(void);
void target_swd_deinit(void);

/* Set the SWCLK divider for the given target clock in kHz. */
void target_swd_set_clk_freq(uint freq_khz);
/* Bit counts in the range 1..256. */
void target_swd_write_bits(uint bit_count, uint32_t data_byte);
uint32_t target_swd_read_bits(uint bit_count);
void target_swd_hiz_clocks(uint bit_count);
void target_swd_read_mode(void);
void target_swd_write_mode(void);

/* nRESET control (the target's run line, inverted on this board). */
void target_swd_assert_reset(bool state);
int target_swd_reset_level(void);

#endif
