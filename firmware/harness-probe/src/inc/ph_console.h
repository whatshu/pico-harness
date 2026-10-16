/* SPDX-License-Identifier: MIT */
#ifndef PH_CONSOLE_H
#define PH_CONSOLE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Work console bridge (core1 side of the Debug<->Work pico_harness link).
 *
 * The host talks to the Work console through the PH protocol (ConsoleTx /
 * ConsoleData messages on PH Terminal, see ph_terminal.c); this module turns
 * those into the pico_harness line protocol over the Work PH_FW_UART:
 *
 *   - PLAIN mode: bytes pass through raw (legacy plain-PH_FW_UART Work firmware).
 *   - HARNESS mode (after an idle Hello is observed): short console messages
 *     are framed as PH `0x02` console TX; Work `0x90` short output is lifted
 *     to ConsoleData events; baud changes run the prepare/verify/commit
 *     transaction (0x03/0x04/0x05). Work `0x91` bulk announcements and the
 *     SPI read-back are handled in the bulk phase (P3).
 *
 * Runs entirely on core1; call ph_console_pump() from the worker loop.
 */

#define PH_CONSOLE_SHORT 128u
#define PH_CONSOLE_FRAME_MAX (PH_CONSOLE_SHORT + 10u)

typedef enum {
    PH_CONSOLE_PLAIN,
    PH_CONSOLE_HARNESS,
} ph_console_mode_t;

void ph_console_init(void);

/* core1: send one low-level pico_harness frame on the Work PH_FW_UART (used by the
 * bulk data plane for `0x06` prepares). */
void ph_console_uart_send(uint8_t kind, const uint8_t *payload, size_t size);

/* core1: advance PH_FW_UART RX parsing, Hello detection and the baud transaction.
 * Call once per worker iteration after ph_fw_target_serial_poll(). */
void ph_console_pump(void);

/* core1: write one console message to Work. No-op when the Work rail is not
 * ready. */
void ph_console_write(const uint8_t *data, size_t len);

/* core1: set the Work PH_FW_UART baud. In harness mode this runs the pico_harness
 * prepare/verify/commit transaction; in plain mode it reconfigures the PH_FW_UART
 * directly. Returns true (with *actual set) on success and false on fnothing
 * state that would block (caller should retry later or report). */
bool ph_console_baud(uint32_t baud, uint32_t *actual);

ph_console_mode_t ph_console_mode(void);

/* core1: the currently applied Work UART baud rate. */
uint32_t ph_console_baud_get(void);


void ph_console_reset(void);

#endif