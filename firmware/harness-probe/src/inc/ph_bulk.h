/* SPDX-License-Identifier: MIT */
#ifndef PH_BULK_H
#define PH_BULK_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Bulk RX ring (host -> Debug payload, raw). WORK_SPI forwarding lands here
 * too in a later phase; for now payloads are buffered and integrity-checked. */
#define PH_BULK_RING 4096u
#define PH_BULK_TIMEOUT_US 2000000u

/* Route constants kept in step with picoharness.v1.Route. */
#define PH_ROUTE_DEBUG 0
#define PH_ROUTE_WORK_SPI 1

void ph_bulk_init(void);

/* core0: drain vendor OUT ("PH Terminal Bulk") into the RX ring. Bytes are
 * only accepted while a bulk RX transaction is active. */
void ph_bulk_pump_rx(void);

/* core0: flush any pending bulk TX (vendor IN) toward the host. No-op until
 * the read-back path (WORK -> host) is built. */
void ph_bulk_flush_tx(void);

/* core1: drive an active bulk RX transaction to completion (buffer payload,
 * CRC check, emit BulkTxDone). Call once per worker iteration. */
void ph_bulk_worker(void);

/* core1: begin a bulk RX transaction. Accepts PH_ROUTE_DEBUG only in this
 * phase; request size must fit the ring and no transaction may be running. */
bool ph_bulk_rx_begin(uint32_t route, uint32_t length, uint32_t crc);

bool ph_bulk_busy(void);

/* core1: complete an active transaction: emit BulkTxDone(ok,crc) on the PH
 * terminal channel and release the data plane. */
void ph_bulk_done(bool ok, uint32_t crc);

/* any core on USB reset/unmount: drop state. */
void ph_bulk_reset(void);

#endif