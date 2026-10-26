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
 *                     registers directly; it reads (ph_fw_target_work_mv(),
 *                     ph_fw_target_io_ready(), gpio) and applies (ph_fw_target_power_set,
 *                     ph_fw_target_io_set_ready()).
 *
 *   target_io.c       the IO layer.  Every register, PIO, DMA and gpio
 *                     operation, split into four engines (serial, swd, led,
 *                     power/measurement/SPI).  See the file header there.
 *
 * Both files share this header, so any module talks to the target through the
 * target_* API and never reaches into pins itself.
 */

#ifndef PH_FW_TARGET_H
#define PH_FW_TARGET_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "pico/stdlib.h"
#include "FreeRTOS.h"
#include "task.h"
#include "pico_dualcore_config.h"

/* ------------------------------------------------------------------------
 * Debug helpers.  Dummy expansion when not enabled; ph_fw_probe_info / ph_fw_probe_debug
 * / ph_fw_probe_dump differ only in intent, not behaviour.
 * ------------------------------------------------------------------------ */

#if false
#define ph_fw_probe_info(format, args...) \
    do {                            \
        vTaskSuspendAll();          \
        printf(format, ##args);     \
        xTaskResumeAll();           \
    } while (0)
#else
#define ph_fw_probe_info(format, ...) ((void)0)
#endif

#if false
#define ph_fw_probe_debug(format, args...) \
    do {                             \
        vTaskSuspendAll();           \
        printf(format, ##args);      \
        xTaskResumeAll();            \
    } while (0)
#else
#define ph_fw_probe_debug(format, ...) ((void)0)
#endif

#if false
#define ph_fw_probe_dump(format, args...) \
    do {                            \
        vTaskSuspendAll();          \
        printf(format, ##args);     \
        xTaskResumeAll();           \
    } while (0)
#else
#define ph_fw_probe_dump(format, ...) ((void)0)
#endif

enum ph_fw_led_mode {
    PH_FW_LED_OFF, /* USB suspended or not configured */
    PH_FW_LED_IDLE, /* USB ready, no SWD target seen yet */
    PH_FW_LED_TARGET, /* SWD target detected, idle / monitoring */
    PH_FW_LED_CAPTURE, /* GPIO capture running */
    PH_FW_LED_ERROR, /* persistent fault (power bad, watchdog, etc.) */
};

/* ------------------------------------------------------------------------
 * Control layer (target_control.c)
 * ------------------------------------------------------------------------ */

/* Poll the Work rail health and drive the IO readiness gate.  Called from
 * core 0's board-monitoring loop every ~5 ms. */
void ph_fw_target_control_poll(void);

/* Power the Work rail up or down; sequences the enable/run/boot controls. */
void ph_fw_target_power_set(bool enabled);

/* ------------------------------------------------------------------------
 * IO layer: power / measurement / SPI (target_io.c)
 * ------------------------------------------------------------------------ */

/* Bring up every controller owned by the IO layer.  Call once, first, on
 * core 0 before any task starts. */
void ph_fw_target_io_init(void);

/* Is the Work rail powered and healthy, i.e. may any Work pin be driven? */
bool ph_fw_target_io_ready(void);

/* Apply the control layer's readiness decision: gate the cross-domain drive. */
void ph_fw_target_io_set_ready(bool ready);

/* Latest Work rail voltage in millivolts (converts the ADC sample). */
uint16_t ph_fw_target_work_mv(void);

/* Change the function selection of a pad without touching its output enable,
 * so it can never briefly drive an unpowered Work pin. */
void ph_fw_target_gpio_function(uint pin, gpio_function_t function);

/* Core 1 owns SPI and the supplied buffers until poll completes or cancel.
 * poll: 0 = pending, 1 = complete, -1 = failed/idle. Neither call waits. */
bool ph_fw_target_spi_start(const uint8_t *tx, uint8_t *rx, uint16_t length);
int ph_fw_target_spi_poll(void);
void ph_fw_target_spi_cancel(void);

/* INA237 power monitor readout: shunt/bus voltage, current and power (uV/uA/uW). */
bool ph_fw_target_power_measure(int32_t values[4]);

/* ------------------------------------------------------------------------
 * IO layer: compact serial bridge (DMA PH_FW_UART; USB identity in ph_fw_usb_serial)
 * ------------------------------------------------------------------------ */

/* iSerialNumber string in the USB device descriptor; NUL terminated after
 * ph_fw_target_serial_init(). */
extern char ph_fw_usb_serial[];

void ph_fw_target_serial_init(void);
bool ph_fw_target_serial_poll(void);
bool ph_fw_target_serial_config(uint8_t channel, uint32_t baud, uint8_t data_bits, uint8_t stop_bits,
                          uint8_t parity, uint32_t *actual_baud);
size_t ph_fw_target_serial_write(uint8_t channel, const uint8_t *data, size_t length);
size_t ph_fw_target_serial_read(uint8_t channel, uint8_t *data, size_t length);
bool ph_fw_target_serial_status(uint8_t channel, uint16_t *rx_available, uint16_t *tx_space,
                          uint32_t *rx_overruns);

/* ------------------------------------------------------------------------
 * IO layer: system-status LED
 * ------------------------------------------------------------------------ */

/* Any module may pick the state the LED should convey; ph_fw_control_thread calls
 * ph_fw_target_led_tick() once per loop to redraw the pattern.  The engine owns
 * the LED gpio, so callers never reach into the pins directly. */
void ph_fw_target_led_init(void);
void ph_fw_target_led_tick(void);
void ph_fw_target_led_set_mode(enum ph_fw_led_mode mode);
enum ph_fw_led_mode ph_fw_target_led_get_mode(void);

/* ------------------------------------------------------------------------
 * IO layer: SWD PIO engine (bit stream behind the CMSIS-DAP transfer)
 * ------------------------------------------------------------------------ */

/* Load the PIO program and bring the state machine up (idempotent). */
void ph_fw_target_swd_init(void);
void ph_fw_target_swd_deinit(void);

/* Set the SWCLK divider for the given target clock in kHz. */
void ph_fw_target_swd_set_clk_freq(uint freq_khz);
/* Bit counts in the range 1..256. */
void ph_fw_target_swd_write_bits(uint bit_count, uint32_t data_byte);
uint32_t ph_fw_target_swd_read_bits(uint bit_count);
void ph_fw_target_swd_hiz_clocks(uint bit_count);
void ph_fw_target_swd_read_mode(void);
void ph_fw_target_swd_write_mode(void);

/* nRESET control (the target's run line, inverted on this board). */
void ph_fw_target_swd_assert_reset(bool state);
int ph_fw_target_swd_reset_level(void);

#endif
