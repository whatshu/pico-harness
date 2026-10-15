/* SPDX-License-Identifier: MIT */
#ifndef PICO_HARNESS_TEST_FAKE_SDK_H
#define PICO_HARNESS_TEST_FAKE_SDK_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef unsigned int uint;
typedef struct { uint32_t fr; } uart_hw_t;
typedef struct { uart_hw_t hw; } uart_inst_t;
extern uart_inst_t test_uart;
#define uart0 (&test_uart)
#define UART_PARITY_NONE 0u
#define UART_UARTFR_BUSY_BITS 1u
#define GPIO_FUNC_UART 2u
#define GPIO_FUNC_PWM 4u
#define GPIO_OVERRIDE_NORMAL 0u
#define GPIO_OVERRIDE_LOW 2u
#define GPIO_OVERRIDE_HIGH 3u
#define clk_peri 0u
#define clk_sys 1u

typedef struct { float divider; uint16_t wrap; } pwm_config;
uint32_t clock_get_hz(uint clock);
uint32_t uart_init(uart_inst_t *uart, uint32_t baud);
uint32_t uart_set_baudrate(uart_inst_t *uart, uint32_t baud);
void uart_set_hw_flow(uart_inst_t *uart, bool cts, bool rts);
void uart_set_format(uart_inst_t *uart, uint data, uint stop, uint parity);
bool uart_is_readable(uart_inst_t *uart);
bool uart_is_writable(uart_inst_t *uart);
uint8_t uart_getc(uart_inst_t *uart);
void uart_putc_raw(uart_inst_t *uart, uint8_t byte);
uart_hw_t *uart_get_hw(uart_inst_t *uart);
uint64_t time_us_64(void);
void gpio_set_function(uint pin, uint function);
void gpio_set_outover(uint pin, uint override);
uint pwm_gpio_to_slice_num(uint pin);
pwm_config pwm_get_default_config(void);
void pwm_config_set_clkdiv(pwm_config *config, float divider);
void pwm_config_set_wrap(pwm_config *config, uint16_t wrap);
void pwm_init(uint slice, const pwm_config *config, bool enabled);
void pwm_set_gpio_level(uint pin, uint16_t level);
void pwm_set_enabled(uint slice, bool enabled);

#endif
