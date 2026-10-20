/* SPDX-License-Identifier: MIT */
#ifndef PH_TERMINAL_H
#define PH_TERMINAL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Derived from ph-bridge/proto/ph.proto (nanopb). Typedef aliases so firmware code
 * reads ph_* without the picoharness_v1_ qualification. */
#include "ph.pb.h"

typedef picoharness_v1_PhMessage ph_msg_t;
typedef picoharness_v1_GetInfo ph_get_info_t;
typedef picoharness_v1_GetStatus ph_get_status_t;
typedef picoharness_v1_PowerSet ph_power_set_t;
typedef picoharness_v1_ResetPulse ph_reset_pulse_t;
typedef picoharness_v1_BootselSet ph_bootsel_set_t;
typedef picoharness_v1_EnterBootsel ph_enter_bootsel_t;
typedef picoharness_v1_Measure ph_measure_t;
typedef picoharness_v1_GpioRead ph_gpio_read_t;
typedef picoharness_v1_GpioDiag ph_gpio_diag_t;
typedef picoharness_v1_DebugBootsel ph_debug_bootsel_t;
typedef picoharness_v1_ConsoleTx ph_console_tx_t;
typedef picoharness_v1_ConsoleData ph_console_data_t;
typedef picoharness_v1_TermConfig ph_term_config_t;
typedef picoharness_v1_TermStatus ph_term_status_t;
typedef picoharness_v1_BulkTxBegin ph_bulk_tx_begin_t;
typedef picoharness_v1_BulkTxDone ph_bulk_tx_done_t;
typedef picoharness_v1_Status ph_status_t;
typedef picoharness_v1_Route ph_route_t;

#define ph_get_info_msg picoharness_v1_GetInfo_msg
#define ph_get_status_msg picoharness_v1_GetStatus_msg
#define ph_power_set_msg picoharness_v1_PowerSet_msg
#define ph_reset_pulse_msg picoharness_v1_ResetPulse_msg
#define ph_bootsel_set_msg picoharness_v1_BootselSet_msg
#define ph_enter_bootsel_msg picoharness_v1_EnterBootsel_msg
#define ph_measure_msg picoharness_v1_Measure_msg
#define ph_gpio_read_msg picoharness_v1_GpioRead_msg
#define ph_gpio_diag_msg picoharness_v1_GpioDiag_msg
#define ph_debug_bootsel_msg picoharness_v1_DebugBootsel_msg
#define ph_console_tx_msg picoharness_v1_ConsoleTx_msg
#define ph_console_data_msg picoharness_v1_ConsoleData_msg
#define ph_term_config_msg picoharness_v1_TermConfig_msg
#define ph_term_status_msg picoharness_v1_TermStatus_msg
#define ph_bulk_tx_begin_msg picoharness_v1_BulkTxBegin_msg
#define ph_bulk_tx_done_msg picoharness_v1_BulkTxDone_msg
#define ph_bulk_rx_avail_msg picoharness_v1_BulkRxAvail_msg
#define ph_bulk_rx_begin_msg picoharness_v1_BulkRxBegin_msg
#define ph_bulk_rx_done_msg picoharness_v1_BulkRxDone_msg
#define ph_message_msg picoharness_v1_PhMessage_msg

/* Status code aliases (picoharness.v1.Status). */
#define PH_STATUS_OK picoharness_v1_Status_OK
#define PH_STATUS_ERR_FRAME picoharness_v1_Status_ERR_FRAME
#define PH_STATUS_ERR_CMD picoharness_v1_Status_ERR_CMD
#define PH_STATUS_ERR_ARG picoharness_v1_Status_ERR_ARG
#define PH_STATUS_ERR_BUSY picoharness_v1_Status_ERR_BUSY
#define PH_STATUS_ERR_CRC picoharness_v1_Status_ERR_CRC
#define PH_STATUS_ERR_ROUTE picoharness_v1_Status_ERR_ROUTE
#define PH_STATUS_ERR_NOTREADY picoharness_v1_Status_ERR_NOTREADY

/* One encoded PH message (length prefix + body) must fit this. The largest
 * payload is ConsoleTx/ConsoleData `bytes` at 128 B; 512 leaves ample room. */
#define PH_MSG_MAX 512u
/* Length-prefix parse: a varint may need up to 5 bytes. */
#define PH_VARINT_MAX 5u

void ph_terminal_init(void);

/* core0: read CDC0 RX and parse length-delimited frames, submit whole
 * messages to the core1 mailbox, then flush pending TX (CDC0 + bulk IN).
 * Returns the number of bytes consumed from the CDC (for backpressure). */
uint32_t ph_terminal_pump(void);

/* core1: fire the message executor if a job is pending. Returns true if a
 * message was consumed. */
bool ph_terminal_worker(void);

/* core1: append message bytes for CDC0 TX. Blocking until there is ring
 * space (bounded by PH_TX_RING). */
void ph_terminal_send(const uint8_t *bytes, size_t len);

/* core1: read a full encoded PhMessage from payload[len] and dispatch. */
void ph_msg_execute(const uint8_t *payload, size_t len);

/* Build and send a reply/event envelope. status and (optionally) one body
 * member; seq echoes the request, 0 for unsolicited events. */
void ph_send_status(uint32_t seq, ph_status_t status);
void ph_send_body(uint32_t seq, ph_status_t status, pb_size_t body_tag,
                  const void *body, size_t body_size);

/* core1: emit a ConsoleData event (Work console short output). */
void ph_console_data_emit(const uint8_t *bytes, size_t len);

/* any core on USB reset/unmount: drop state, return to idle. */
void ph_terminal_reset(void);

#endif