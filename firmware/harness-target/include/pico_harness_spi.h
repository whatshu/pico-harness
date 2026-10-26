/* SPDX-License-Identifier: MIT */
#ifndef PICO_HARNESS_SPI_H
#define PICO_HARNESS_SPI_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Shared low-level engine for protocol and hardware test fixtures.
 * Application projects should normally use pico_harness.h. */
#define PICO_HARNESS_SPI_BLOCK 2048u
void pico_harness_spi_init(void);
bool pico_harness_spi_arm(const uint8_t *transmit, size_t size, uint8_t mode);
bool pico_harness_spi_done(void);
void pico_harness_spi_read(uint8_t *receive, size_t size);
void pico_harness_spi_abort(void);
#endif
