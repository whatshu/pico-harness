/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/clocks.h"
#include "pico/stdlib.h"

#include "ph_console.h"
#include "ph_terminal.h"
#include "target.h"
#include "util.h"

/* Frame layout and timing constants mirror firmware/harness-target
 * `src/pico_harness.c` (the Work side of this link); the Debug core is the
 * "host" of that link. */
#define PH_KIND_HELLO 0x81
#define PH_KIND_CONSOLE_TX 0x02
#define PH_KIND_CONSOLE_DATA 0x90
#define PH_KIND_BULK_ANNOUNCE 0x91
#define PH_KIND_BAUD_PREPARE 0x03
#define PH_KIND_BAUD_PREPARE_ACK 0x83
#define PH_KIND_BAUD_VERIFY 0x04
#define PH_KIND_BAUD_VERIFY_ACK 0x84
#define PH_KIND_BAUD_COMMIT 0x05
#define PH_KIND_BAUD_COMMIT_ACK 0x85

#define BAUD_SWITCH_DELAY_US 100000u
#define BAUD_PREPARE_TIMEOUT_US 1000000u
#define FRAME_IDLE_MIN_US 20000u

#define BAUD_MIN 300u
#define BAUD_MAX() (clock_get_hz(clk_peri) / 16u)

static ph_console_mode_t mode;
static uint32_t current_baud;
static uint16_t seq;

#define BAUDS_IDLE 0
#define BAUDS_PREPARE 1
#define BAUDS_VERIFY 2
#define BAUDS_COMMIT 3
static uint8_t baud_state;
static uint32_t baud_target, baud_previous;
static uint64_t baud_t0;        /* start of current transaction */
static uint64_t baud_switch_at; /* wall-clock instant to switch rates */

static uint8_t rx_frame[PH_CONSOLE_FRAME_MAX];
static size_t rx_len;
static uint64_t rx_last_at;

static void ph_console_baud_rollback(void);

static uint16_t next_seq(void)
{
    seq = (uint16_t)(seq + 1u);
    return seq ? seq : (seq = 1u);
}

static void send(uint8_t kind, uint16_t sequence, const uint8_t *payload, size_t size)
{
    uint8_t bytes[PH_CONSOLE_FRAME_MAX];
    bytes[0] = 'P';
    bytes[1] = 'H';
    bytes[2] = 1;
    bytes[3] = kind;
    bytes[4] = (uint8_t)sequence;
    bytes[5] = (uint8_t)(sequence >> 8);
    bytes[6] = (uint8_t)size;
    bytes[7] = (uint8_t)(size >> 8);
    if (size)
        memcpy(bytes + 8, payload, size);
    uint16_t crc = crc16_ccitt(bytes + 2, size + 6);
    bytes[size + 8] = (uint8_t)crc;
    bytes[size + 9] = (uint8_t)(crc >> 8);
    if (target_serial_write(0, bytes, size + 10) < size + 10)
        return; /* Work UART fifo full; the write is partial at worst */
}

static uint64_t rollback_lease_us(void)
{
    uint64_t wire_us = ((uint64_t)PH_CONSOLE_SHORT * 10 * 1000000u + current_baud - 1u) /
                       ((uint64_t)current_baud ? current_baud : 1u);
    uint64_t timeout = wire_us + 500000u;
    return timeout > 1500000u ? timeout : 1500000u;
}

/* ------------------------------------------------------------------ */
/* RX frame parsing (mirrors pico_harness parse_frames)               */
/* ------------------------------------------------------------------ */

static void handle_frame(uint8_t kind, uint16_t frame_seq, const uint8_t *payload, size_t size);
static void consume(size_t total)
{
    if (total >= rx_len) {
        rx_len = 0;
        return;
    }
    memmove(rx_frame, rx_frame + total, rx_len - total);
    rx_len -= total;
}

static void parse_frames(bool expire)
{
    while (rx_len) {
        if (rx_frame[0] != 'P' || (rx_len >= 2 && rx_frame[1] != 'H')) {
            consume(1);
            continue;
        }
        if (rx_len < 8) {
            if (!expire)
                return;
            consume(1);
            continue;
        }
        size_t size = rx_frame[6] | ((size_t)rx_frame[7] << 8);
        if (rx_frame[2] != 1 || size > PH_CONSOLE_SHORT) {
            consume(1);
            continue;
        }
        if (rx_len < size + 10) {
            if (!expire)
                return;
            consume(1);
            continue;
        }
        uint16_t got = rx_frame[size + 8] | ((uint16_t)rx_frame[size + 9] << 8);
        if (crc16_ccitt(rx_frame + 2, size + 6) != got) {
            consume(1);
            continue;
        }
        handle_frame(rx_frame[3], (uint16_t)(rx_frame[4] | (rx_frame[5] << 8)),
                     rx_frame + 8, size);
        consume(size + 10);
    }
}

static void handle_frame(uint8_t kind, uint16_t frame_seq, const uint8_t *payload, size_t size)
{
    (void)frame_seq;
    switch (kind) {
    case PH_KIND_HELLO:
        /* Capabilities announcement: [1, 7, 0, 8]. Only idle Work announces. */
        if (size == 4 && payload[0] == 1)
            mode = PH_CONSOLE_HARNESS;
        break;
    case PH_KIND_CONSOLE_DATA:
        ph_console_data_emit(payload, size);
        break;
    case PH_KIND_BULK_ANNOUNCE:
        /* SPI read-back prepare is the bulk phase (P3). A 2s prepared
         * transfer on Work expires on its own; ignore for now. */
        break;
    case PH_KIND_BAUD_PREPARE_ACK:
        if (baud_state == BAUDS_PREPARE && size == 4) {
            uint32_t echoed;
            memcpy(&echoed, payload, 4);
            if (echoed == baud_target) {
                /* Work switches ~100ms after IT received the prepare; we must
                 * switch after that. The 0x03->0x83 round trip costs ~1.2 ms
                 * at 115200, so anchor on the ACK instant (not our send) and
                 * add a small margin. */
                baud_switch_at = time_us_64() + BAUD_SWITCH_DELAY_US + 5000u;
                uint64_t now = time_us_64();
                if (now < baud_switch_at)
                    busy_wait_until(baud_switch_at);
                uint32_t actual;
                target_serial_config(0, baud_target, 8, 1, 0, &actual);
                current_baud = actual;
                rx_len = 0; /* bytes buffered under the old rate are stale */
                send(PH_KIND_BAUD_VERIFY, next_seq(), NULL, 0);
                baud_t0 = time_us_64();
                baud_state = BAUDS_VERIFY;
            }
        }
        break;
    case PH_KIND_BAUD_VERIFY_ACK:
        if (baud_state == BAUDS_VERIFY && size == 4) {
            uint32_t actual;
            memcpy(&actual, payload, 4);
            uint32_t err = actual > baud_target ? actual - baud_target : baud_target - actual;
            if (err * 100u <= baud_target * 2u) {
                send(PH_KIND_BAUD_COMMIT, next_seq(), NULL, 0);
                baud_t0 = time_us_64();
                baud_state = BAUDS_COMMIT;
            } else {
                ph_console_baud_rollback();
            }
        }
        break;
    case PH_KIND_BAUD_COMMIT_ACK:
        if (baud_state == BAUDS_COMMIT && size == 4) {
            uint32_t actual;
            memcpy(&actual, payload, 4);
            current_baud = actual;
            baud_state = BAUDS_IDLE;
        }
        break;
    default:
        break;
    }
}

/* Restore the previous rate after a failed or unfulfilled transaction. Work
 * rolls back on its own when the commit lease expires; we follow it. */
static void ph_console_baud_rollback(void)
{
    ph_console_data_emit((const uint8_t *)"[baud] rollback\n", 16);
    uint32_t actual;
    target_serial_config(0, baud_previous, 8, 1, 0, &actual);
    current_baud = actual;
    rx_len = 0;
    baud_state = BAUDS_IDLE;
}

/* ------------------------------------------------------------------ */

void ph_console_init(void)
{
    mode = PH_CONSOLE_PLAIN;
    seq = 0;
    rx_len = 0;
    baud_state = BAUDS_IDLE;
    uint32_t baud = 115200;
    target_serial_config(0, baud, 8, 1, 0, &baud);
    current_baud = baud;
}

void ph_console_pump(void)
{
    uint8_t chunk[32];
    for (int i = 0; i < 16; ++i) {
        size_t n = target_serial_read(0, chunk, sizeof(chunk));
        if (!n)
            break;
        rx_last_at = time_us_64();
        /* PLAIN mode: Work output is raw application bytes — pass through,
         * while still scanning for an idle Hello to upgrade to HARNESS. */
        if (mode == PH_CONSOLE_PLAIN)
            ph_console_data_emit(chunk, n);
        for (size_t j = 0; j < n && rx_len < PH_CONSOLE_FRAME_MAX; ++j)
            rx_frame[rx_len++] = chunk[j];
        parse_frames(false);
    }

    /* Expire a partially filled frame after ~4 characters / 20 ms. */
    if (rx_len && time_us_64() - rx_last_at >= FRAME_IDLE_MIN_US)
        parse_frames(true);

    /* Advance the baud transaction: timeouts / leases. */
    if (baud_state) {
        uint64_t lease = rollback_lease_us();
        uint64_t elapsed = time_us_64() - baud_t0;
        if ((baud_state == BAUDS_PREPARE && elapsed > BAUD_PREPARE_TIMEOUT_US) ||
            (baud_state != BAUDS_PREPARE && elapsed > lease)) {
            ph_console_baud_rollback();
        }
    }

    /* PLAIN -> HARNESS discovery is done in handle_frame on Hello; additionally
     * re-announcements while HARNESS keep the mode sticky. Nothing to do. */
}

void ph_console_write(const uint8_t *data, size_t len)
{
    if (!target_io_ready() || len == 0 || len > PH_CONSOLE_SHORT)
        return;
    if (mode == PH_CONSOLE_HARNESS) {
        send(PH_KIND_CONSOLE_TX, next_seq(), data, len);
    } else {
        target_serial_write(0, data, len);
    }
}

bool ph_console_baud(uint32_t baud, uint32_t *actual)
{
    if (baud < BAUD_MIN || baud > BAUD_MAX())
        return false;

    if (mode == PH_CONSOLE_HARNESS) {
        /* Requesting (essentially) the current rate is a no-op. */
        uint32_t low = baud - baud / 50u, high = baud + baud / 50u;
        if (current_baud >= low && current_baud <= high) {
            *actual = current_baud;
            return true;
        }
        if (baud_state)
            return false; /* a transaction is already running */
        baud_previous = current_baud;
        baud_target = baud;
        uint32_t le = baud;
        send(PH_KIND_BAUD_PREPARE, next_seq(), (const uint8_t *)&le, 4);
        baud_t0 = time_us_64();
        baud_state = BAUDS_PREPARE;
        /* Wait out the transaction (100 ms switch + verify/commit). */
        uint64_t deadline = time_us_64() + BAUD_PREPARE_TIMEOUT_US + rollback_lease_us();
        while (baud_state != BAUDS_IDLE && time_us_64() < deadline) {
            ph_console_pump();
            tight_loop_contents();
        }
        if (baud_state != BAUDS_IDLE)
            ph_console_baud_rollback();
        /* Success = the applied rate landed within 2% of the request (UART
         * dividers round, so e.g. 125000000/271 = 460829 for 460800). */
        uint32_t err = current_baud > baud_target ? current_baud - baud_target
                                                  : baud_target - current_baud;
        if (err * 100u > baud_target * 2u)
            return false;
        *actual = current_baud;
        return true;
    }

    /* PLAIN mode — reconfigure directly. */
    uint32_t applied = baud;
    if (!target_serial_config(0, baud, 8, 1, 0, &applied))
        return false;
    current_baud = applied;
    *actual = applied;
    return true;
}

ph_console_mode_t ph_console_mode(void)
{
    return mode;
}

void ph_console_reset(void)
{
    mode = PH_CONSOLE_PLAIN;
    seq = 0;
    rx_len = 0;
    baud_state = BAUDS_IDLE;
    uint32_t baud = 115200;
    target_serial_config(0, baud, 8, 1, 0, &baud);
    current_baud = baud;
}