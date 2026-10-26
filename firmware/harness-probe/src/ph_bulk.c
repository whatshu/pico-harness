/* SPDX-License-Identifier: MIT */
#include <string.h>
#include "hardware/sync.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "ph_bulk.h"
#include "ph_console.h"
#include "ph_terminal.h"
#include "ph_fw_target.h"
#include "ph_fw_util.h"

/* Ring indices are monotonic positions, masked only when indexing the array;
 * the serial RX path in target_io.c uses the same scheme. */
static uint8_t rx_ring[PH_BULK_RING] __attribute__((aligned(4)));
static volatile uint32_t rx_head; /* core0 producer */
static volatile uint32_t rx_tail; /* core1 consumer */

/* Transaction state: owned by core1, read (volatile) by core0 so the RX pump
 * stops accepting stray bytes unless a transaction expects them. */
static volatile uint8_t tx_active;
static volatile bool reset_pending;
static uint8_t tx_route;
static uint32_t tx_len, tx_crc, tx_started_us;

/* WORK_SPI forward FSM. */
#define PICO_HARNESS_BLOCK 2048u
#define PH_BULK_TX_PREPARE 0x06 /* Debug -> Work: prepare SPI RX */
#define PH_BULK_TX_ACK 0x86     /* Work -> Debug: prepared */
#define PH_BULK_RX_PREPARE 0x08 /* Debug -> Work: prepare read-back (later) */
#define PH_BULK_CRC 0x87        /* Work -> Debug: per-block CRC16 */
#define PH_WORK_SPI_IDLE 0
#define PH_WORK_SPI_PREPARE 1
#define PH_WORK_SPI_WAIT_ACK 2
#define PH_WORK_SPI_TRANSFER 3
#define PH_WORK_SPI_WAIT_CRC 4
#define PH_WORK_SPI_WAIT_DMA 5
static uint8_t ws_state;
static bool block_crc_ready;
static uint8_t block_crc[2];
static uint8_t block_tx[PICO_HARNESS_BLOCK];
static uint8_t block_rx[PICO_HARNESS_BLOCK];
static uint16_t block_len;
static uint32_t total_sent;
static uint32_t crc_accum;
static uint64_t ws_t0;

static void publish(void)
{
    __dmb();
}

void ph_bulk_init(void)
{
    rx_head = rx_tail = 0;
    tx_active = 0;
    ws_state = PH_WORK_SPI_IDLE;
    reset_pending = false;
}

bool ph_bulk_busy(void)
{
    return tx_active != 0 || reset_pending;
}

bool ph_bulk_rx_begin(uint32_t route, uint32_t length, uint32_t crc)
{
    if (tx_active || reset_pending || length == 0)
        return false;
    if (route != PH_ROUTE_DEBUG && route != PH_ROUTE_WORK_SPI)
        return false;
    if (route == PH_ROUTE_DEBUG && length > PH_BULK_RING)
        return false;
    if (route == PH_ROUTE_WORK_SPI && ph_console_mode() != PH_CONSOLE_HARNESS)
        return false;
    tx_route = (uint8_t)route;
    tx_len = length;
    tx_crc = crc;
    tx_started_us = time_us_32();
    if (route == PH_ROUTE_WORK_SPI) {
        total_sent = 0;
        crc_accum = ~0u;
        ws_state = PH_WORK_SPI_PREPARE;
        block_len = 0;
        ws_t0 = time_us_32();
    }
    publish();
    tx_active = 1;
    publish();
    return true;
}

void ph_bulk_pump_rx(void)
{
    if (reset_pending || !tx_active)
        return;
    __dmb();
    uint32_t space = PH_BULK_RING - (rx_head - rx_tail);
    if (!space)
        return;
    uint32_t offset = rx_head % PH_BULK_RING;
    uint32_t contig = PH_BULK_RING - offset;
    if (contig > space)
        contig = space;
    uint32_t n = tud_vendor_read(rx_ring + offset, contig);
    if (n) {
        publish();
        rx_head += n;
    }
}

static uint32_t crc32c_ring(uint32_t offset, uint32_t count)
{
    offset %= PH_BULK_RING;
    uint32_t first = MIN(count, PH_BULK_RING - offset);
    uint32_t crc = ph_fw_crc32c_update(~0u, rx_ring + offset, first);
    crc = ph_fw_crc32c_update(crc, rx_ring, count - first);
    return ~crc;
}

/* Copy `want` bytes from the RX ring into `dst` and advance the tail. Call
 * only when at least `want` bytes are produced. */
static void ring_pull(uint8_t *dst, uint32_t want)
{
    __dmb();
    uint32_t offset = rx_tail % PH_BULK_RING;
    uint32_t first = MIN(want, PH_BULK_RING - offset);
    memcpy(dst, rx_ring + offset, first);
    memcpy(dst + first, rx_ring, want - first);
    __dmb();
    rx_tail += want;
}

void ph_bulk_worker(void)
{
    if (reset_pending) {
        /* Core 0 has stopped pumping. Only the DMA owner may release the
         * block buffers and reset the producer/consumer positions. */
        __dmb();
        ph_fw_target_spi_cancel();
        rx_head = rx_tail = 0;
        tx_active = 0;
        ws_state = PH_WORK_SPI_IDLE;
        __dmb();
        reset_pending = false;
        return;
    }
    if (!tx_active) {
        /* USB reset may have discarded a transaction. Stop its DMA on the
         * owning core before another worker iteration can reuse buffers. */
        ph_fw_target_spi_cancel();
        return;
    }

    if (tx_route == PH_ROUTE_DEBUG) {
        uint32_t produced = rx_head - rx_tail;
        if (produced < tx_len) {
            if (time_us_32() - tx_started_us > PH_BULK_TIMEOUT_US)
                ph_bulk_done(false, 0);
            return;
        }
        __dmb();
        uint32_t crc = crc32c_ring(rx_tail, tx_len);
        ph_bulk_done(crc == tx_crc, crc);
        rx_tail += tx_len;
        return;
    }

    /* WORK_SPI: forward blocks over the pico_harness SPI path. */
    switch (ws_state) {
    case PH_WORK_SPI_PREPARE: {
        uint32_t remaining = tx_len - total_sent;
        block_len = remaining < PICO_HARNESS_BLOCK ? (uint16_t)remaining : PICO_HARNESS_BLOCK;
        if (rx_head - rx_tail < block_len) {
            if (time_us_32() - ws_t0 > PH_BULK_TIMEOUT_US)
                ph_bulk_done(false, 0);
            return; /* wait for the block to arrive */
        }
        ring_pull(block_tx, block_len);
        block_crc_ready = false;
        crc_accum = ph_fw_crc32c_update(crc_accum, block_tx, block_len);
        uint8_t le[2] = {(uint8_t)block_len, (uint8_t)(block_len >> 8)};
        ph_console_uart_send(PH_BULK_TX_PREPARE, le, 2);
        ws_state = PH_WORK_SPI_WAIT_ACK;
        ws_t0 = time_us_32();
        return;
    }
    case PH_WORK_SPI_WAIT_ACK:
        if (time_us_32() - ws_t0 > 500000u)
            ph_bulk_done(false, 0);
        return; /* ph_bulk_on_uart will move us to TRANSFER */
    case PH_WORK_SPI_TRANSFER:
        if (!ph_fw_target_spi_start(block_tx, block_rx, block_len)) {
            ph_bulk_done(false, 0);
            return;
        }
        ws_state = PH_WORK_SPI_WAIT_DMA;
        return;
    case PH_WORK_SPI_WAIT_DMA: {
        int result = ph_fw_target_spi_poll();
        if (!result)
            return;
        if (result < 0) {
            ph_bulk_done(false, 0);
            return;
        }
        ws_state = PH_WORK_SPI_WAIT_CRC;
        ws_t0 = time_us_32();
        /* UART is serviced before this FSM. Work may already have replied
         * between DMA completion and our next poll. Keep that CRC until now. */
        if (block_crc_ready)
            ph_bulk_on_uart(PH_BULK_CRC, 0, block_crc, sizeof(block_crc));
        return;
    }
    case PH_WORK_SPI_WAIT_CRC:
        if (time_us_32() - ws_t0 > 500000u)
            ph_bulk_done(false, 0);
        return; /* ph_bulk_on_uart verifies CRC and advances or completes */
    default:
        return;
    }
}

void ph_bulk_on_uart(uint8_t kind, uint16_t seq, const uint8_t *payload, size_t size)
{
    (void)seq;
    if (reset_pending || !tx_active || tx_route != PH_ROUTE_WORK_SPI)
        return;
    if (kind == PH_BULK_TX_ACK && ws_state == PH_WORK_SPI_WAIT_ACK && size == 2) {
        uint16_t announced = (uint16_t)(payload[0] | (payload[1] << 8));
        if (announced == block_len)
            ws_state = PH_WORK_SPI_TRANSFER;
    } else if (kind == PH_BULK_CRC && ws_state == PH_WORK_SPI_WAIT_DMA && size == 2) {
        memcpy(block_crc, payload, sizeof(block_crc));
        block_crc_ready = true;
    } else if (kind == PH_BULK_CRC && ws_state == PH_WORK_SPI_WAIT_CRC && size == 2) {
        uint16_t got = (uint16_t)(payload[0] | (payload[1] << 8));
        if (got != ph_fw_crc16_ccitt(block_tx, block_len)) {
            ph_bulk_done(false, 0);
            return;
        }
        total_sent += block_len;
        if (total_sent == tx_len) {
            crc_accum = ~crc_accum; /* finish CRC-32C */
            ph_bulk_done(crc_accum == tx_crc, crc_accum);
        } else {
            ws_state = PH_WORK_SPI_PREPARE;
        }
    }
}

void ph_bulk_done(bool ok, uint32_t crc)
{
    ph_bulk_tx_done_t done = picoharness_v1_BulkTxDone_init_default;
    done.ok = ok;
    done.crc_recv = crc;
    ph_send_body(0, PH_STATUS_OK, picoharness_v1_PhMessage_bulk_tx_done_tag,
                 &done, sizeof(done));
    publish();
    tx_active = 0;
    ws_state = PH_WORK_SPI_IDLE;
    publish();
}

void ph_bulk_flush_tx(void)
{
    /* Read-back direction (WORK -> host) is implemented in the next phase;
     * vendor IN stays idle for now. */
}

void ph_bulk_reset(void)
{
    /* Called by Core 0 on USB reset, between USB pump iterations. */
    __dmb();
    reset_pending = true;
}
