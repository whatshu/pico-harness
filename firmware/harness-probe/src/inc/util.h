/* SPDX-License-Identifier: MIT */
#ifndef PDBG_UTIL_H
#define PDBG_UTIL_H

#include <stddef.h>
#include <stdint.h>

/* Stringifier: PDBG_STR(x) -> "x" */
#define PDBG_STR_HELPER(x) #x
#define PDBG_STR(x) PDBG_STR_HELPER(x)

uint16_t crc16_ccitt(const uint8_t *data, size_t length);
uint32_t crc32c(const uint8_t *data, size_t length);
uint32_t normalize_gpio(uint32_t raw);

#endif
