/* SPDX-License-Identifier: MIT */
#include "hardware/sync.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "ph_bulk.h"
#include "ph_terminal.h"

/* Ring indices are monotonic positions, masked only when indexing the array;
 * the serial RX path in target_io.c uses the same scheme. */
static uint8_t rx_ring[PH_BULK_RING] __attribute__((aligned(4)));
static volatile uint32_t rx_head; /* core0 producer */
static volatile uint32_t rx_tail; /* core1 consumer */

/* Transaction state: owned by core1, read (volatile) by core0 so the RX pump
 * stops accepting stray bytes unless a transaction expects them. */
static volatile uint8_t tx_active;
static uint32_t tx_len, tx_crc, tx_started_us;

static void publish(void)
{
    __dmb();
}

void ph_bulk_init(void)
{
    rx_head = rx_tail = 0;
    tx_active = 0;
}

bool ph_bulk_busy(void)
{
    return tx_active != 0;
}

bool ph_bulk_rx_begin(uint32_t route, uint32_t length, uint32_t crc)
{
    if (tx_active || route != PH_ROUTE_DEBUG || length == 0 || length > PH_BULK_RING)
        return false;
    tx_len = length;
    tx_crc = crc;
    tx_started_us = time_us_32();
    publish();
    tx_active = 1;
    publish();
    return true;
}

void ph_bulk_pump_rx(void)
{
    if (!tx_active)
        return;
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

/* Byte-wise CRC-32C (Castagnoli) over a possibly wrapping ring region. A 4 KiB
 * region is a few hundred microseconds; this runs on core1 between jobs. */
static uint32_t crc32c_ring(uint32_t offset, uint32_t count)
{
    uint32_t crc = ~0u;
    for (uint32_t i = 0; i < count; ++i) {
        crc ^= rx_ring[(offset + i) % PH_BULK_RING];
        for (int bit = 0; bit < 8; ++bit)
            crc = (crc >> 1) ^ (0x82f63b78u & -(int32_t)(crc & 1));
    }
    return ~crc;
}

void ph_bulk_worker(void)
{
    if (!tx_active)
        return;

    uint32_t produced = rx_head - rx_tail;
    if (produced < tx_len) {
        if (time_us_32() - tx_started_us > PH_BULK_TIMEOUT_US) {
            /* Never received the promised payload. */
            ph_bulk_done(false, 0);
        }
        return;
    }

    uint32_t crc = crc32c_ring(rx_tail, tx_len);
    ph_bulk_done(crc == tx_crc, crc);
    rx_tail += tx_len;
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
    publish();
}

void ph_bulk_flush_tx(void)
{
    /* Read-back direction (WORK -> host) is not implemented yet; vendor IN
     * simply stays idle. */
}

void ph_bulk_reset(void)
{
    rx_head = rx_tail = 0;
    tx_active = 0;
}