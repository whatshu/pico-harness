/* SPDX-License-Identifier: MIT */
#include "pico_harness.h"
#include "pico_harness_spi.h"
#include "hardware/clocks.h"
#include "hardware/pwm.h"
#include "hardware/uart.h"
#include "pico/stdlib.h"
#include <math.h>
#include <string.h>

#define UART uart0
#define SHORT_MAX 128u
#define TX_CAPACITY 1024u
#define FRAME_CAPACITY (SHORT_MAX + 10u)
#define BAUD_SWITCH_DELAY_US 100000u
#define BAUD_PREPARE_TIMEOUT_US 1000000u
#define BAUD_ROLLBACK_MIN_US 1500000u
#define BAUD_HANDSHAKE_BYTES 64u
#define BAUD_HANDSHAKE_MARGIN_US 500000u
#define FRAME_IDLE_MIN_US 20000u

static pico_harness_receive_fn on_receive;
static void *receive_context;
static uint8_t tx_ring[TX_CAPACITY], frame[FRAME_CAPACITY];
static size_t tx_head, tx_tail, frame_size;
static uint8_t bulk_rx[PICO_HARNESS_SPI_BLOCK], bulk_tx[PICO_HARNESS_SPI_BLOCK];
static size_t bulk_tx_size, bulk_size;
static uint16_t bulk_sequence;
static bool bulk_receiving, bulk_active;
static uint32_t baud, previous_baud, requested_baud;
static uint64_t switch_at, switch_deadline, rollback_at, bulk_deadline, hello_at;
static uint64_t frame_last_byte_at;

static uint64_t baud_rollback_timeout(void)
{
    /* Verify and commit must fit even at 300 baud. At ordinary rates this
     * retains the 1.5-second lease used by existing hosts. */
    uint64_t wire_us = ((uint64_t)BAUD_HANDSHAKE_BYTES * 10 * 1000000 + baud - 1) / baud;
    uint64_t timeout = wire_us + BAUD_HANDSHAKE_MARGIN_US;
    return timeout > BAUD_ROLLBACK_MIN_US ? timeout : BAUD_ROLLBACK_MIN_US;
}

static uint16_t crc16(const uint8_t *data, size_t size)
{
    uint16_t crc = 0xffff;
    for (size_t i = 0; i < size; ++i) {
        crc ^= (uint16_t)data[i] << 8;
        for (uint bit = 0; bit < 8; ++bit)
            crc = (crc << 1) ^ ((crc & 0x8000) ? 0x1021 : 0);
    }
    return crc;
}

static bool packet(uint8_t kind, uint16_t sequence, const uint8_t *data, size_t size)
{
    if (size > SHORT_MAX || TX_CAPACITY - (tx_head - tx_tail) < size + 10)
        return false;
    uint8_t bytes[FRAME_CAPACITY] = {'P', 'H', 1, kind, sequence, sequence >> 8, size, size >> 8};
    if (size)
        memcpy(bytes + 8, data, size);
    uint16_t crc = crc16(bytes + 2, size + 6);
    bytes[size + 8] = crc;
    bytes[size + 9] = crc >> 8;
    for (size_t i = 0; i < size + 10; ++i)
        tx_ring[tx_head++ % TX_CAPACITY] = bytes[i];
    return true;
}

static bool announce_hello(uint16_t sequence)
{
    const uint8_t capabilities[] = {1, 7, 0, 8}; /* version, UART+SPI+baud, block=2048 */
    return packet(0x81, sequence, capabilities, sizeof(capabilities));
}

bool pico_harness_write(const void *data, size_t size)
{
    if (!data || !size || size > PICO_HARNESS_SPI_BLOCK || switch_at || rollback_at)
        return false;
    /* Keep room for control replies even when the application writes continuously. */
    if (TX_CAPACITY - (tx_head - tx_tail) < (size <= SHORT_MAX ? size : 2) + 10 + FRAME_CAPACITY)
        return false;
    if (size <= SHORT_MAX)
        return packet(0x90, 0, data, size);
    if (bulk_tx_size)
        return false;
    const uint8_t available[2] = {size, size >> 8};
    if (!packet(0x91, 0, available, sizeof(available)))
        return false;
    memcpy(bulk_tx, data, size);
    bulk_tx_size = size;
    return true;
}

static void command(uint8_t kind, uint16_t sequence, const uint8_t *data, size_t size)
{
    if (kind == 1 && !size) {
        announce_hello(sequence);
    } else if (kind == 2 && size) {
        if (on_receive)
            on_receive(data, size, receive_context);
        packet(0x82, sequence, NULL, 0);
    } else if (kind == 3 && size == 4 && !switch_at && !rollback_at && !bulk_active) {
        uint32_t requested;
        memcpy(&requested, data, 4);
        if (requested >= 300 && requested <= clock_get_hz(clk_peri) / 16 &&
            packet(0x83, sequence, data, 4)) {
            previous_baud = baud;
            requested_baud = requested;
            /* The ACK finishes at the old rate before the agreed switch. */
            uint64_t now = time_us_64();
            switch_at = now + BAUD_SWITCH_DELAY_US;
            switch_deadline = now + BAUD_PREPARE_TIMEOUT_US;
        }
    } else if (kind == 4 && !size) {
        packet(0x84, sequence, (uint8_t *)&baud, 4);
    } else if (kind == 5 && !size && rollback_at &&
               packet(0x85, sequence, (uint8_t *)&baud, 4)) {
        rollback_at = 0;
    } else if (kind == 6 && size == 2 && !bulk_active && !switch_at && !rollback_at) {
        bulk_size = data[0] | ((size_t)data[1] << 8);
        if (bulk_size && bulk_size <= PICO_HARNESS_SPI_BLOCK) {
            bulk_receiving = true;
            bulk_active = pico_harness_spi_arm(NULL, bulk_size, 0);
            bulk_sequence = sequence;
            bulk_deadline = time_us_64() + 2000000;
            packet(0x86, sequence, data, 2);
        }
    } else if (kind == 8 && !size && !bulk_active && !switch_at && !rollback_at) {
        uint8_t response[2] = {bulk_tx_size, bulk_tx_size >> 8};
        if (bulk_tx_size) {
            bulk_size = bulk_tx_size;
            bulk_receiving = false;
            bulk_active = pico_harness_spi_arm(bulk_tx, bulk_size, 0);
            bulk_sequence = sequence;
            bulk_deadline = time_us_64() + 2000000;
        }
        packet(0x88, sequence, response, 2);
    }
}

static void discard_frame_prefix(size_t size)
{
    frame_size -= size;
    memmove(frame, frame + size, frame_size);
}

/* Retain the next valid PH frame after noise, a bad CRC, or an expired partial
 * frame. The buffer and scan are bounded by FRAME_CAPACITY. */
static void parse_frames(bool expire_partial)
{
    while (frame_size) {
        if (frame[0] != 'P' || (frame_size >= 2 && frame[1] != 'H')) {
            discard_frame_prefix(1);
            continue;
        }
        if (frame_size < 8) {
            if (!expire_partial)
                return;
            discard_frame_prefix(1);
            continue;
        }
        size_t size = frame[6] | ((size_t)frame[7] << 8);
        if (frame[2] != 1 || size > SHORT_MAX) {
            discard_frame_prefix(1);
            continue;
        }
        if (frame_size < size + 10) {
            if (!expire_partial)
                return;
            discard_frame_prefix(1);
            continue;
        }
        uint16_t received = frame[size + 8] | ((uint16_t)frame[size + 9] << 8);
        if (crc16(frame + 2, size + 6) != received) {
            discard_frame_prefix(1);
            continue;
        }
        command(frame[3], frame[4] | ((uint16_t)frame[5] << 8), frame + 8, size);
        discard_frame_prefix(size + 10);
    }
}

void pico_harness_init(pico_harness_receive_fn receive, void *context)
{
    tx_head = tx_tail = frame_size = 0;
    bulk_tx_size = bulk_size = 0;
    bulk_active = false;
    switch_at = switch_deadline = rollback_at = 0;
    on_receive = receive;
    receive_context = context;
    baud = uart_init(UART, 115200);
    uart_set_hw_flow(UART, false, false);
    uart_set_format(UART, 8, 1, UART_PARITY_NONE);
    gpio_set_function(12, GPIO_FUNC_UART);
    gpio_set_function(13, GPIO_FUNC_UART);
    pico_harness_spi_init();
    pico_harness_spi_abort();
    command(1, 0, NULL, 0);
    hello_at = time_us_64() + 1000000;
}

void pico_harness_poll(void)
{
    /* Four character times accommodate low baud rates; the floor permits USB
     * packet gaps at higher rates without treating a normal fragment as noise. */
    uint64_t idle_timeout = ((uint64_t)40 * 1000000 + baud - 1) / baud;
    if (idle_timeout < FRAME_IDLE_MIN_US)
        idle_timeout = FRAME_IDLE_MIN_US;
    if (frame_size && time_us_64() - frame_last_byte_at >= idle_timeout)
        parse_frames(true);
    for (uint budget = 0; budget < 64 && uart_is_readable(UART); ++budget) {
        uint8_t byte = uart_getc(UART);
        frame_last_byte_at = time_us_64();
        frame[frame_size++] = byte;
        parse_frames(false);
    }
    for (uint budget = 0; budget < 64 && tx_tail != tx_head && uart_is_writable(UART); ++budget)
        uart_putc_raw(UART, tx_ring[tx_tail++ % TX_CAPACITY]);

    uint64_t now = time_us_64();
    /* Allow a newly opened desktop to identify a silent application's console. */
    if (now >= hello_at && !switch_at && !rollback_at && tx_head == tx_tail &&
        !(uart_get_hw(UART)->fr & UART_UARTFR_BUSY_BITS) && announce_hello(0)) {
        hello_at = now + 1000000;
    }
    if (switch_at && now >= switch_deadline) {
        /* Never execute an old prepare later, after the host has given up. */
        switch_at = switch_deadline = 0;
    } else if (switch_at && now >= switch_at && tx_head == tx_tail &&
        !(uart_get_hw(UART)->fr & UART_UARTFR_BUSY_BITS)) {
        baud = uart_set_baudrate(UART, requested_baud);
        switch_at = switch_deadline = 0;
        rollback_at = now + baud_rollback_timeout();
        frame_size = 0;
    }
    if (rollback_at && now >= rollback_at) {
        baud = uart_set_baudrate(UART, previous_baud);
        rollback_at = 0;
        frame_size = 0;
        tx_tail = tx_head;
    }
    if (bulk_active && pico_harness_spi_done()) {
        pico_harness_spi_read(bulk_rx, bulk_size);
        uint16_t crc = crc16(bulk_receiving ? bulk_rx : bulk_tx, bulk_size);
        pico_harness_spi_abort();
        bulk_active = false;
        uint8_t response[2] = {crc, crc >> 8};
        packet(0x87, bulk_sequence, response, 2);
        if (bulk_receiving && on_receive)
            on_receive(bulk_rx, bulk_size, receive_context);
        if (!bulk_receiving)
            bulk_tx_size = 0;
    } else if (bulk_active && now >= bulk_deadline) {
        pico_harness_spi_abort();
        bulk_active = false;
    }
}

uint32_t pico_harness_baud(void)
{
    return baud;
}

uint32_t pico_harness_pwm(uint8_t pin, uint32_t frequency_hz, float duty)
{
    if (pin > 29 || pin == 12 || pin == 13 || (pin >= 18 && pin <= 21) ||
        !frequency_hz || !isfinite(duty) || !(duty >= 0.0f && duty <= 1.0f))
        return 0;
    uint32_t clock = clock_get_hz(clk_sys);
    float divider = (float)clock / ((float)frequency_hz * 65536.0f);
    if (divider < 1.0f)
        divider = 1.0f;
    if (divider > 255.9375f)
        return 0;
    divider = (uint32_t)(divider * 16.0f + 0.999f) / 16.0f;
    uint32_t period = (uint32_t)((float)clock / (divider * frequency_hz) + 0.5f);
    if (period < 2 || period > 65536)
        return 0;
    gpio_set_function(pin, GPIO_FUNC_PWM);
    uint slice = pwm_gpio_to_slice_num(pin);
    pwm_config config = pwm_get_default_config();
    pwm_config_set_clkdiv(&config, divider);
    pwm_config_set_wrap(&config, period - 1);
    pwm_init(slice, &config, false);
    uint32_t level = (uint32_t)(duty * period + 0.5f);
    if (level >= period)
        level = period - 1;
    pwm_set_gpio_level(pin, (uint16_t)level);
    /* A 16-bit compare cannot represent 65536. Pad overrides guarantee the
     * endpoints for every period and are cleared by the next fractional duty. */
    gpio_set_outover(pin, duty == 0.0f ? GPIO_OVERRIDE_LOW :
                          duty == 1.0f ? GPIO_OVERRIDE_HIGH : GPIO_OVERRIDE_NORMAL);
    pwm_set_enabled(slice, true);
    return (uint32_t)((float)clock / (divider * period));
}
