/* SPDX-License-Identifier: MIT */
/* Simple interactive terminal over the PicoHarness console, MicroPython REPL
 * style: control the board LED, configure GPIO / PWM / a second UART / the ADC
 * input, and invoke them. Every Console TX frame is input; echo, responses and
 * the prompt are queued and drained from the main loop through
 * pico_harness_write(). Pins 12, 13 and 18..21 carry the harness link and are
 * reserved, as are the library's own PWM constraints. */
#include "pico_harness.h"
#include "hardware/adc.h"
#include "hardware/clocks.h"
#include "hardware/pwm.h"
#include "hardware/uart.h"
#include "hardware/watchdog.h"
#include "pico/stdlib.h"
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define LINE_MAX 200u
#define REPLY_MAX 2048u
#define REPLY_CHUNK_MAX 128u
#define UART1_TX 4u
#define UART1_RX 5u

static char reply[REPLY_MAX];
static size_t reply_len, reply_sent;
static bool uart1_active;
static bool adc_started;
static bool pending_reboot;
static uint64_t reboot_at;

static void reply_fmt(const char *fmt, ...)
{
    size_t slack = sizeof(reply) - reply_len;
    if (!slack)
        return;
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(reply + reply_len, slack, fmt, ap);
    va_end(ap);
    if (n > 0)
        reply_len += n < (int)slack ? (size_t)n : slack;
}

static bool pin_reserved(unsigned int pin)
{
    return pin == 12 || pin == 13 || (pin >= 18 && pin <= 21);
}

static bool parse_pin(const char *text, unsigned int *pin)
{
    char *end;
    unsigned long value = strtoul(text, &end, 10);
    if (end == text || *end || value > 29)
        return false;
    *pin = (unsigned int)value;
    return true;
}

static void terminal_prompt(void)
{
    reply_fmt("harness> ");
}

static void cmd_help(void)
{
    reply_fmt("help                  this help\r\n");
    reply_fmt("led on|off|toggle     control the board LED\r\n");
    reply_fmt("led                   report the board LED\r\n");
    reply_fmt("gpio <pin> out        configure <pin> 0..29 as output\r\n");
    reply_fmt("gpio <pin> in [up|down]  configure as input, optional pull\r\n");
    reply_fmt("gpio <pin> set|clear|toggle  drive an output pin\r\n");
    reply_fmt("gpio <pin> read       read a pin (input level or output state)\r\n");
    reply_fmt("pwm <pin> <hz> <duty> PWM output, duty 0..1\r\n");
    reply_fmt("pwm <pin> off         stop PWM on <pin>\r\n");
    reply_fmt("uart <hz>             second UART on GP%u TX / GP%u RX\r\n", UART1_TX, UART1_RX);
    reply_fmt("uart tx <text>        send <text> on the second UART\r\n");
    reply_fmt("uart off              release the second UART\r\n");
    reply_fmt("adc <pin>             read ADC, <pin> 26..28\r\n");
    reply_fmt("info                  clock, harness baud, uptime\r\n");
    reply_fmt("sleep <ms>            wait 1..10000 ms\r\n");
    reply_fmt("reboot                reset the Pico\r\n");
    reply_fmt("pins 12, 13, 18..21 carry the harness link and are reserved\r\n");
}

static void cmd_led(char *args)
{
    char *save = NULL;
    char *action = strtok_r(args, " \t", &save);
    if (!action) {
        reply_fmt("led: GP%u = %u\r\n", PICO_DEFAULT_LED_PIN,
                  gpio_get(PICO_DEFAULT_LED_PIN) ? 1 : 0);
    } else if (!strcmp(action, "on")) {
        gpio_put(PICO_DEFAULT_LED_PIN, 1);
        reply_fmt("led: GP%u = 1\r\n", PICO_DEFAULT_LED_PIN);
    } else if (!strcmp(action, "off")) {
        gpio_put(PICO_DEFAULT_LED_PIN, 0);
        reply_fmt("led: GP%u = 0\r\n", PICO_DEFAULT_LED_PIN);
    } else if (!strcmp(action, "toggle")) {
        gpio_put(PICO_DEFAULT_LED_PIN, !gpio_get(PICO_DEFAULT_LED_PIN));
        reply_fmt("led: GP%u = %u\r\n", PICO_DEFAULT_LED_PIN,
                  gpio_get(PICO_DEFAULT_LED_PIN) ? 1 : 0);
    } else {
        reply_fmt("error: led expects on, off or toggle\r\n");
    }
}

static void cmd_gpio(char *args)
{
    char *save = NULL;
    char *text = strtok_r(args, " \t", &save);
    unsigned int pin;
    if (!text || !parse_pin(text, &pin) || pin_reserved(pin)) {
        reply_fmt("error: 'gpio <pin> out|in [up|down]|set|clear|toggle|read'\r\n");
        return;
    }
    char *op = strtok_r(NULL, " \t", &save);
    if (!op) {
        reply_fmt("error: 'gpio <pin> out|in [up|down]|set|clear|toggle|read'\r\n");
        return;
    }
    if (!strcmp(op, "out")) {
        gpio_init(pin);
        gpio_set_dir(pin, true);
        reply_fmt("gpio: GP%u output\r\n", pin);
    } else if (!strcmp(op, "in")) {
        gpio_init(pin);
        gpio_set_dir(pin, false);
        char *pull = strtok_r(NULL, " \t", &save);
        if (pull && !strcmp(pull, "up")) {
            gpio_pull_up(pin);
            reply_fmt("gpio: GP%u input pull-up\r\n", pin);
        } else if (pull && !strcmp(pull, "down")) {
            gpio_pull_down(pin);
            reply_fmt("gpio: GP%u input pull-down\r\n", pin);
        } else {
            gpio_disable_pulls(pin);
            reply_fmt("gpio: GP%u input%s\r\n", pin, pull ? " (unknown pull)" : "");
        }
    } else if (!strcmp(op, "set")) {
        gpio_init(pin);
        gpio_set_dir(pin, true);
        gpio_put(pin, 1);
        reply_fmt("gpio: GP%u = 1\r\n", pin);
    } else if (!strcmp(op, "clear")) {
        gpio_init(pin);
        gpio_set_dir(pin, true);
        gpio_put(pin, 0);
        reply_fmt("gpio: GP%u = 0\r\n", pin);
    } else if (!strcmp(op, "toggle")) {
        gpio_init(pin);
        gpio_set_dir(pin, true);
        gpio_put(pin, !gpio_get_out_level(pin));
        reply_fmt("gpio: GP%u = %u\r\n", pin, gpio_get_out_level(pin) ? 1 : 0);
    } else if (!strcmp(op, "read")) {
        gpio_init(pin);
        gpio_set_dir(pin, false);
        gpio_disable_pulls(pin);
        reply_fmt("gpio: GP%u = %u\r\n", pin, gpio_get(pin) ? 1 : 0);
    } else {
        reply_fmt("error: unknown gpio op '%s'\r\n", op);
    }
}

static void cmd_pwm(char *args)
{
    char *save = NULL;
    char *text = strtok_r(args, " \t", &save);
    unsigned int pin;
    if (!text || !parse_pin(text, &pin) || pin_reserved(pin)) {
        reply_fmt("error: 'pwm <pin> <hz> <duty>' or 'pwm <pin> off'\r\n");
        return;
    }
    char *op = strtok_r(NULL, " \t", &save);
    if (!op) {
        reply_fmt("error: 'pwm <pin> <hz> <duty>' or 'pwm <pin> off'\r\n");
        return;
    }
    if (!strcmp(op, "off")) {
        uint slice = pwm_gpio_to_slice_num(pin);
        pwm_set_enabled(slice, false);
        gpio_set_function(pin, GPIO_FUNC_SIO);
        gpio_set_dir(pin, true);
        gpio_put(pin, 0);
        reply_fmt("pwm: GP%u stopped\r\n", pin);
        return;
    }
    char *duty_text = strtok_r(NULL, " \t", &save);
    char *end;
    unsigned long frequency = strtoul(op, &end, 10);
    if (end == op || *end || !frequency) {
        reply_fmt("error: invalid frequency '%s'\r\n", op);
        return;
    }
    if (!duty_text) {
        reply_fmt("error: 'pwm <pin> <hz> <duty>'\r\n");
        return;
    }
    float duty = strtof(duty_text, &end);
    if (end == duty_text || *end || duty < 0.0f || duty > 1.0f) {
        reply_fmt("error: duty must be a number in 0..1\r\n");
        return;
    }
    uint32_t actual = pico_harness_pwm(pin, (uint32_t)frequency, duty);
    if (!actual) {
        reply_fmt("error: PWM rejected (frequency out of range or pin busy)\r\n");
        return;
    }
    reply_fmt("pwm: GP%u %lu Hz duty %u/1000 -> actual %lu Hz\r\n", pin, frequency,
              (unsigned)(duty * 1000.0f + 0.5f), (unsigned long)actual);
}

static void cmd_uart(char *args)
{
    char *save = NULL;
    char *sub = strtok_r(args, " \t", &save);
    if (!sub) {
        reply_fmt("error: 'uart <hz> | uart on <hz> | uart tx <text> | uart off'\r\n");
        return;
    }
    if (!strcmp(sub, "tx")) {
        if (!uart1_active) {
            reply_fmt("error: second UART is not configured (use 'uart <hz>')\r\n");
            return;
        }
        while (*save == ' ' || *save == '\t')
            ++save;
        size_t size = strlen(save);
        while (size && (save[size - 1] == '\r' || save[size - 1] == '\n'))
            --size;
        if (!size) {
            reply_fmt("uart: nothing to send\r\n");
            return;
        }
        for (size_t i = 0; i < size; ++i)
            uart_putc_raw(uart1, (uint8_t)save[i]);
        reply_fmt("uart: sent %lu bytes\r\n", (unsigned long)size);
        return;
    }
    if (!strcmp(sub, "off")) {
        if (!uart1_active) {
            reply_fmt("uart: second UART is not active\r\n");
            return;
        }
        uart_deinit(uart1);
        gpio_set_function(UART1_TX, GPIO_FUNC_SIO);
        gpio_set_dir(UART1_TX, true);
        gpio_put(UART1_TX, 0);
        gpio_set_function(UART1_RX, GPIO_FUNC_SIO);
        gpio_set_dir(UART1_RX, false);
        uart1_active = false;
        reply_fmt("uart: released GP%u / GP%u\r\n", UART1_TX, UART1_RX);
        return;
    }
    char *freq_text = sub;
    if (!strcmp(sub, "on")) {
        freq_text = strtok_r(NULL, " \t", &save);
        if (!freq_text) {
            reply_fmt("error: 'uart on <hz>'\r\n");
            return;
        }
    }
    char *end;
    unsigned long baud = strtoul(freq_text, &end, 10);
    if (end == freq_text || *end || baud < 300 || baud > 2000000) {
        reply_fmt("error: baud must be 300..2000000\r\n");
        return;
    }
    uart_init(uart1, (uint32_t)baud);
    uart_set_hw_flow(uart1, false, false);
    uart_set_format(uart1, 8, 1, UART_PARITY_NONE);
    gpio_set_function(UART1_TX, GPIO_FUNC_UART);
    gpio_set_function(UART1_RX, GPIO_FUNC_UART);
    uart1_active = true;
    reply_fmt("uart: UART1 @ %lu baud (GP%u TX / GP%u RX)\r\n", baud, UART1_TX, UART1_RX);
}

static void cmd_adc(char *args)
{
    char *save = NULL;
    char *text = strtok_r(args, " \t", &save);
    unsigned int pin;
    if (!text || !parse_pin(text, &pin) || pin < 26 || pin > 28) {
        reply_fmt("error: 'adc <pin>', pin 26..28\r\n");
        return;
    }
    if (!adc_started) {
        adc_init();
        adc_started = true;
    }
    adc_gpio_init(pin);
    adc_select_input(pin - 26);
    uint16_t value = adc_read();
    reply_fmt("adc: GP%u = %u / 4095 (%u mV)\r\n", pin, value,
              (unsigned)((uint32_t)value * 3300u / 4095u));
}

static void cmd_info(char *args)
{
    (void)args;
    reply_fmt("firmware: harness-terminal (harness-target pico_harness)\r\n");
    reply_fmt("clock: %lu Hz, harness baud %lu\r\n", (unsigned long)clock_get_hz(clk_sys),
              (unsigned long)pico_harness_baud());
    reply_fmt("uptime: %llu ms\r\n", (unsigned long long)(time_us_64() / 1000));
}

static void cmd_sleep(char *args)
{
    char *save = NULL;
    char *text = strtok_r(args, " \t", &save);
    char *end;
    unsigned long ms;
    if (!text) {
        reply_fmt("error: 'sleep <ms>'\r\n");
        return;
    }
    ms = strtoul(text, &end, 10);
    if (end == text || *end || !ms || ms > 10000) {
        reply_fmt("error: ms must be 1..10000\r\n");
        return;
    }
    busy_wait_us(ms * 1000);
    reply_fmt("sleep: waited %lu ms\r\n", ms);
}

static void cmd_reboot(char *args)
{
    (void)args;
    reply_fmt("reboot: resetting...\r\n");
    pending_reboot = true;
    reboot_at = time_us_64() + 100000;
}

static void dispatch(char *verb, char *args)
{
    if (!strcmp(verb, "help")) {
        cmd_help();
    } else if (!strcmp(verb, "led")) {
        cmd_led(args);
    } else if (!strcmp(verb, "gpio")) {
        cmd_gpio(args);
    } else if (!strcmp(verb, "pwm")) {
        cmd_pwm(args);
    } else if (!strcmp(verb, "uart")) {
        cmd_uart(args);
    } else if (!strcmp(verb, "adc")) {
        cmd_adc(args);
    } else if (!strcmp(verb, "info")) {
        cmd_info(args);
    } else if (!strcmp(verb, "sleep")) {
        cmd_sleep(args);
    } else if (!strcmp(verb, "reboot")) {
        cmd_reboot(args);
    } else {
        reply_fmt("error: unknown command '%s' (try 'help')\r\n", verb);
    }
    if (!pending_reboot)
        terminal_prompt();
}

/* Echo the reconstructed line (so typed input is visible) and run it. Frames
 * split on '\n'; a trailing fragment without a newline is still a line, which
 * matches a desktop Console that sends each command as one message. */
static void run_line(const uint8_t *data, size_t size)
{
    while (size && (data[size - 1] == '\r' || data[size - 1] == '\n'))
        --size;
    if (!size) {
        terminal_prompt();
        return;
    }
    char line[LINE_MAX + 1];
    bool truncated = size > LINE_MAX;
    size_t copy = truncated ? LINE_MAX : size;
    memcpy(line, data, copy);
    line[copy] = '\0';
    reply_fmt("%s\r\n", line);
    if (truncated)
        reply_fmt("error: line too long, truncated\r\n");
    char *save = NULL;
    char *verb = strtok_r(line, " \t", &save);
    if (verb) {
        dispatch(verb, save);
    } else {
        terminal_prompt();
    }
}

static void terminal_accept(const uint8_t *data, size_t size, void *context)
{
    (void)context;
    size_t start = 0;
    for (size_t i = 0; i < size; ++i) {
        if (data[i] == '\n') {
            run_line(data + start, i - start);
            start = i + 1;
        }
    }
    if (start < size)
        run_line(data + start, size - start);
}

/* Send the queued reply in short (<= 128 B) whole-line chunks over the plain
 * UART path. A single reply of 129+ bytes would be offered over SPI bulk, and
 * a bulk that does not complete blocks every later reply of that size; keeping
 * terminal output on UART makes replies self-sufficient. */
static void flush_reply(void)
{
    while (reply_len > reply_sent) {
        size_t chunk = reply_len - reply_sent;
        if (chunk > REPLY_CHUNK_MAX)
            chunk = REPLY_CHUNK_MAX;
        /* Prefer complete lines so the desktop transcript rows stay intact
         * and each short message ends cleanly. */
        size_t last_line = reply_sent;
        for (size_t i = reply_sent; i < reply_sent + chunk && i < reply_len; ++i) {
            if (reply[i] == '\n')
                last_line = i + 1;
        }
        if (last_line > reply_sent)
            chunk = last_line - reply_sent;
        if (!pico_harness_write((const uint8_t *)reply + reply_sent, chunk))
            return;
        reply_sent += chunk;
    }
    reply_len = reply_sent = 0;
}

int main(void)
{
    pico_harness_init(terminal_accept, NULL);
#ifdef PICO_DEFAULT_LED_PIN
    gpio_init(PICO_DEFAULT_LED_PIN);
    gpio_set_dir(PICO_DEFAULT_LED_PIN, true);
#endif
    reply_fmt("PicoHarness harness-terminal\r\n");
    reply_fmt("type 'help' for commands\r\n");
    terminal_prompt();
    for (;;) {
        pico_harness_poll();
        flush_reply();
        if (pending_reboot && !reply_len && time_us_64() >= reboot_at)
            watchdog_reboot(0, 0, 0);
        tight_loop_contents();
    }
}