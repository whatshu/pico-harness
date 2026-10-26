/* SPDX-License-Identifier: MIT */
#include "ph_fw_util.h"
#include "ph_fw_target.h"

/* Frame CRCs run on core 0 during request parsing and response framing,
 * sharing its time with USB servicing.  Byte tables replace eight polynomial
 * steps per byte without changing the wire format or using mutable state. */
static const uint16_t crc16_table[256] = {
#include "crc16_table.inc"
};
static const uint32_t crc32c_table[256] = {
#include "crc32c_table.inc"
};

uint16_t ph_fw_crc16_ccitt(const uint8_t *data, size_t length)
{
    uint16_t crc = 0xffff;
    while (length--)
        crc = (uint16_t)((crc << 8) ^ crc16_table[((crc >> 8) ^ *data++) & 0xff]);
    return crc;
}

uint32_t ph_fw_crc32c_update(uint32_t crc, const uint8_t *data, size_t length)
{
    while (length--)
        crc = (crc >> 8) ^ crc32c_table[(crc ^ *data++) & 0xff];
    return crc;
}

uint32_t ph_fw_crc32c(const uint8_t *data, size_t length)
{
    return ~ph_fw_crc32c_update(~0u, data, length);
}

/* Remap raw GPIO samples to the protocol's logical channel bit layout. The
 * board routes LA channels to scattered physical pins; PH_FW_GPIO_MAP lists
 * each (physical, logical) pair so the host sees a stable bit map. */
uint32_t ph_fw_normalize_gpio(uint32_t raw)
{
    uint32_t sample = 0;
#define PH_FW_MAP_PIN(debug_pin, work_pin) sample |= ((raw >> debug_pin) & 1u) << work_pin;
    PH_FW_GPIO_MAP(PH_FW_MAP_PIN)
#undef PH_FW_MAP_PIN
    return sample & PH_FW_GPIO_CAPTURE_MASK;
}
