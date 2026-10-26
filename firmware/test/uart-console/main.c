/* SPDX-License-Identifier: MIT */
#include "hardware/uart.h"
#include "pico/stdlib.h"

int main(void)
{
    uart_init(uart0, 115200);
    gpio_set_function(12, GPIO_FUNC_UART);
    gpio_set_function(13, GPIO_FUNC_UART);
    uart_puts(uart0, "UART console ready\r\n");
    for (;;) {
        if (uart_is_readable(uart0)) {
            uart_putc_raw(uart0, uart_getc(uart0));
        }
        tight_loop_contents();
    }
}
