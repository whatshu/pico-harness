/* SPDX-License-Identifier: MIT */
#include "fake_sdk.h"
#include "pico_harness.h"
#include "pico_harness_spi.h"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

/* Fixed wire fixtures, including their CRCs, exercise the production parser. */
static const uint8_t prepare[] = {
    0x50, 0x48, 0x01, 0x03, 0x01, 0x00, 0x04, 0x00,
    0x00, 0x10, 0x0e, 0x00, 0x26, 0x82
};
static const uint8_t prepare_300[] = {
    0x50, 0x48, 0x01, 0x03, 0x01, 0x00, 0x04, 0x00,
    0x2c, 0x01, 0x00, 0x00, 0x06, 0xad
};
static const uint8_t verify[] = {
    0x50, 0x48, 0x01, 0x04, 0x02, 0x00, 0x00, 0x00, 0xde, 0x2f
};
static const uint8_t commit[] = {
    0x50, 0x48, 0x01, 0x05, 0x03, 0x00, 0x00, 0x00, 0x3b, 0xf3
};
static uint8_t uart_rx[2048], uart_tx[8192];
static uint32_t uart_tx_baud[8192];
static size_t rx_head, rx_tail, tx_size;
static uint32_t actual_baud;
static uint64_t now_us;
static uint32_t baud_changes;
static uint16_t gpio_level[30];
static uint gpio_override[30];
static pwm_config slice_config[8];
uart_inst_t test_uart;

uint32_t clock_get_hz(uint clock) { (void)clock; return 125000000; }
uint32_t uart_init(uart_inst_t *uart, uint32_t baud)
{
    (void)uart;
    actual_baud = baud;
    return baud;
}
uint32_t uart_set_baudrate(uart_inst_t *uart, uint32_t baud)
{
    (void)uart;
    actual_baud = baud;
    ++baud_changes;
    return baud;
}
void uart_set_hw_flow(uart_inst_t *uart, bool cts, bool rts)
{ (void)uart; (void)cts; (void)rts; }
void uart_set_format(uart_inst_t *uart, uint data, uint stop, uint parity)
{ (void)uart; (void)data; (void)stop; (void)parity; }
bool uart_is_readable(uart_inst_t *uart)
{ (void)uart; return rx_tail < rx_head; }
bool uart_is_writable(uart_inst_t *uart) { (void)uart; return true; }
uint8_t uart_getc(uart_inst_t *uart) { (void)uart; return uart_rx[rx_tail++]; }
void uart_putc_raw(uart_inst_t *uart, uint8_t byte)
{
    (void)uart;
    assert(tx_size < sizeof(uart_tx));
    uart_tx[tx_size] = byte;
    uart_tx_baud[tx_size++] = actual_baud;
}
uart_hw_t *uart_get_hw(uart_inst_t *uart) { return &uart->hw; }
uint64_t time_us_64(void) { return now_us; }
void gpio_set_function(uint pin, uint function) { (void)pin; (void)function; }
void gpio_set_outover(uint pin, uint override) { gpio_override[pin] = override; }
uint pwm_gpio_to_slice_num(uint pin) { return (pin >> 1) & 7; }
pwm_config pwm_get_default_config(void) { return (pwm_config){1.0f, 0xffff}; }
void pwm_config_set_clkdiv(pwm_config *config, float divider) { config->divider = divider; }
void pwm_config_set_wrap(pwm_config *config, uint16_t wrap) { config->wrap = wrap; }
void pwm_init(uint slice, const pwm_config *config, bool enabled)
{ (void)enabled; slice_config[slice] = *config; }
void pwm_set_gpio_level(uint pin, uint16_t level) { gpio_level[pin] = level; }
void pwm_set_enabled(uint slice, bool enabled) { (void)slice; (void)enabled; }
void pico_harness_spi_init(void) {}
bool pico_harness_spi_arm(const uint8_t *transmit, size_t size, uint8_t mode)
{ (void)transmit; (void)size; (void)mode; return true; }
bool pico_harness_spi_done(void) { return false; }
void pico_harness_spi_read(uint8_t *receive, size_t size) { (void)receive; (void)size; }
void pico_harness_spi_abort(void) {}

static void reset(void)
{
    rx_head = rx_tail = tx_size = 0;
    now_us = 1000;
    baud_changes = 0;
    test_uart.hw.fr = 0;
    pico_harness_init(NULL, NULL);
    pico_harness_poll();
    tx_size = 0; /* Ignore the unsolicited startup Hello. */
}

static void receive_bytes(const uint8_t *bytes, size_t size)
{
    assert(size <= sizeof(uart_rx) - rx_head);
    memcpy(uart_rx + rx_head, bytes, size);
    rx_head += size;
    while (rx_tail < rx_head)
        pico_harness_poll();
    rx_head = rx_tail = 0;
}

static bool has_reply(uint8_t kind)
{
    for (size_t i = 0; i + 3 < tx_size; ++i)
        if (uart_tx[i] == 'P' && uart_tx[i + 1] == 'H' &&
            uart_tx[i + 2] == 1 && uart_tx[i + 3] == kind)
            return true;
    return false;
}

static void baud_change_with_continuous_application_output(void)
{
    reset();
    uint8_t message[128];
    memset(message, 'A', sizeof(message));
    while (pico_harness_write(message, sizeof(message))) {}
    receive_bytes(prepare, sizeof(prepare));
    assert(!pico_harness_write(message, sizeof(message)));
    for (uint i = 0; i < 60; ++i) {
        now_us += 2000;
        pico_harness_poll();
        assert(!pico_harness_write(message, sizeof(message)));
    }
    assert(pico_harness_baud() == 921600);
    assert(has_reply(0x83));
    for (size_t i = 0; i < tx_size; ++i)
        assert(uart_tx_baud[i] == 115200);
    receive_bytes(verify, sizeof(verify));
    assert(has_reply(0x84));
    assert(!pico_harness_write(message, sizeof(message)));
    receive_bytes(commit, sizeof(commit));
    assert(has_reply(0x85));
    assert(pico_harness_write(message, sizeof(message)));
}

static void abandoned_baud_transactions_release_writes(void)
{
    reset();
    receive_bytes(prepare, sizeof(prepare));
    now_us += 100000;
    pico_harness_poll();
    assert(pico_harness_baud() == 921600);
    assert(!pico_harness_write("x", 1));
    now_us += 1500000;
    pico_harness_poll();
    assert(pico_harness_baud() == 115200);
    assert(pico_harness_write("x", 1));

    reset();
    test_uart.hw.fr = UART_UARTFR_BUSY_BITS;
    receive_bytes(prepare, sizeof(prepare));
    now_us += 120000;
    pico_harness_poll();
    assert(!pico_harness_write("x", 1));
    now_us += 880000;
    pico_harness_poll();
    assert(pico_harness_baud() == 115200);
    assert(pico_harness_write("x", 1));
    test_uart.hw.fr = 0;
    now_us += 1000000;
    pico_harness_poll();
    assert(baud_changes == 0); /* An expired prepare cannot switch later. */
}

static void truncated_and_corrupt_frames_preserve_the_next_command(void)
{
    reset();
    const uint8_t partial[] = {'P', 'H', 1, 2, 9, 0, 128, 0, 'X', 'X', 'X', 'X'};
    receive_bytes(partial, sizeof(partial));
    receive_bytes(verify, sizeof(verify));
    assert(!has_reply(0x84));
    now_us += 19999;
    pico_harness_poll();
    assert(!has_reply(0x84));
    ++now_us;
    pico_harness_poll();
    assert(has_reply(0x84));

    reset();
    uint8_t corrupted[30] = {'P', 'H', 1, 2, 9, 0, 20, 0, 'X', 'X', 'X', 'X'};
    memcpy(corrupted + 12, verify, sizeof(verify));
    receive_bytes(corrupted, sizeof(corrupted));
    assert(has_reply(0x84));
}

static void low_baud_byte_fragments_remain_valid(void)
{
    reset();
    receive_bytes(prepare_300, sizeof(prepare_300));
    now_us += 100000;
    pico_harness_poll();
    assert(pico_harness_baud() == 300);
    tx_size = 0;
    for (size_t i = 0; i < sizeof(verify); ++i) {
        now_us += 33334; /* One 8N1 character at 300 baud. */
        receive_bytes(verify + i, 1);
    }
    assert(has_reply(0x84));
}

static void low_baud_commit_and_rollback_allow_the_full_wire_budget(void)
{
    reset();
    receive_bytes(prepare_300, sizeof(prepare_300));
    now_us += 100000;
    pico_harness_poll();
    assert(pico_harness_baud() == 300);
    tx_size = 0;
    for (size_t i = 0; i < sizeof(verify); ++i) {
        now_us += 33334;
        receive_bytes(verify + i, 1);
    }
    assert(has_reply(0x84));
    now_us += 14 * 33334; /* The Verify reply must leave the 300-baud wire. */
    pico_harness_poll();
    now_us += 500000; /* Allow realistic host scheduling before Commit. */
    pico_harness_poll();
    for (size_t i = 0; i < sizeof(commit); ++i) {
        now_us += 33334;
        receive_bytes(commit + i, 1);
    }
    assert(has_reply(0x85));
    assert(pico_harness_baud() == 300);
    assert(pico_harness_write("x", 1));

    reset();
    receive_bytes(prepare_300, sizeof(prepare_300));
    now_us += 100000;
    pico_harness_poll();
    now_us += 2633333; /* One microsecond before the documented 300-baud lease. */
    pico_harness_poll();
    assert(pico_harness_baud() == 300);
    assert(!pico_harness_write("x", 1));
    ++now_us;
    pico_harness_poll();
    assert(pico_harness_baud() == 115200);
    assert(pico_harness_write("x", 1));
}

static void pwm_has_correct_compare_and_exact_endpoints(void)
{
    reset();
    assert(pico_harness_pwm(5, 62500000, 0.5f) == 62500000);
    assert(slice_config[2].wrap == 1 && gpio_level[5] == 1);
    assert(gpio_override[5] == GPIO_OVERRIDE_NORMAL);
    assert(pico_harness_pwm(5, 62500000, 1.0f) == 62500000);
    assert(gpio_override[5] == GPIO_OVERRIDE_HIGH);
    assert(pico_harness_pwm(5, 62500000, 0.0f) == 62500000);
    assert(gpio_override[5] == GPIO_OVERRIDE_LOW);
    assert(pico_harness_pwm(5, 10000, 0.25f) == 10000);
    assert(gpio_level[5] == 3125 && gpio_override[5] == GPIO_OVERRIDE_NORMAL);
    assert(pico_harness_pwm(5, 10000, NAN) == 0);
    assert(pico_harness_pwm(5, 10000, INFINITY) == 0);
    assert(pico_harness_pwm(5, 10000, -0.1f) == 0);
}

static void periodic_hello_waits_for_an_idle_uart(void)
{
    reset();
    test_uart.hw.fr = UART_UARTFR_BUSY_BITS;
    now_us += 1000000;
    pico_harness_poll();
    now_us += 2000000;
    pico_harness_poll();
    assert(tx_size == 0); /* No announcements accumulate behind the busy UART. */
    test_uart.hw.fr = 0;
    pico_harness_poll();
    pico_harness_poll(); /* Drain the announcement just accepted into the ring. */
    assert(has_reply(0x81));
    assert(tx_size == 14); /* One overdue announcement rather than a backlog. */
    tx_size = 0;
    now_us += 999999;
    pico_harness_poll();
    assert(tx_size == 0);
    ++now_us;
    pico_harness_poll();
    pico_harness_poll();
    assert(has_reply(0x81));

    reset();
    uint8_t message[128];
    memset(message, 'A', sizeof(message));
    assert(pico_harness_write(message, sizeof(message)));
    now_us += 1000000;
    pico_harness_poll();
    pico_harness_poll();
    assert(!has_reply(0x81));
    pico_harness_poll();
    assert(tx_size == 138 && !has_reply(0x81));
    pico_harness_poll();
    assert(has_reply(0x81)); /* The application ring drained before Hello queued. */
}

int main(void)
{
    baud_change_with_continuous_application_output();
    abandoned_baud_transactions_release_writes();
    truncated_and_corrupt_frames_preserve_the_next_command();
    low_baud_byte_fragments_remain_valid();
    low_baud_commit_and_rollback_allow_the_full_wire_budget();
    pwm_has_correct_compare_and_exact_endpoints();
    periodic_hello_waits_for_an_idle_uart();
    puts("Work library: 7 portable regression scenarios passed");
    return 0;
}
