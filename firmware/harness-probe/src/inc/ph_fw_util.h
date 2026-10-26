/* SPDX-License-Identifier: MIT */
#ifndef PH_FW_UTIL_H
#define PH_FW_UTIL_H

#include <stddef.h>
#include <stdint.h>

/* Stringifier: PH_FW_STR(x) -> "x" */
#define PH_FW_STR_HELPER(x) #x
#define PH_FW_STR(x) PH_FW_STR_HELPER(x)

uint16_t ph_fw_crc16_ccitt(const uint8_t *data, size_t length);
/* Unfinalized CRC-32C state: seed with ~0u, complement once at completion. */
uint32_t ph_fw_crc32c_update(uint32_t crc, const uint8_t *data, size_t length);
uint32_t ph_fw_crc32c(const uint8_t *data, size_t length);
uint32_t ph_fw_normalize_gpio(uint32_t raw);

#endif
