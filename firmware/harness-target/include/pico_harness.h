/* SPDX-License-Identifier: MIT */
#ifndef PICO_HARNESS_H
#define PICO_HARNESS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Callbacks run inside pico_harness_poll(), never in an interrupt. */
typedef void (*pico_harness_receive_fn)(const uint8_t *data, size_t size, void *context);

/* Fixed carrier wiring: UART0 GP12/13; SPI GP18/19/20/21. */
void pico_harness_init(pico_harness_receive_fn receive, void *context);
void pico_harness_poll(void);

/* Nonblocking. Returns false when the bounded transmit queue is full or a
 * coordinated baud change is pending. Retry the same data from the main loop.
 * <=128 bytes use UART; 129..2048 bytes are offered over SPI/DMA. */
bool pico_harness_write(const void *data, size_t size);
uint32_t pico_harness_baud(void);

/* PWM on a free Work GPIO. Duty is finite and 0..1; returns the actual frequency.
 * The 0% and 100% endpoints are exact; intermediate values are quantized.
 * GPIO12/13/18/19/20/21 are reserved by this library. */
uint32_t pico_harness_pwm(uint8_t pin, uint32_t frequency_hz, float duty);

#ifdef __cplusplus
}
#endif
#endif
