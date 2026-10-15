/* SPDX-License-Identifier: MIT */
#include "util.h"
#include "target.h"

/* Frame CRCs run on core 0 during request parsing and response framing,
 * sharing its time with USB servicing.  Byte tables replace eight polynomial
 * steps per byte without changing the wire format or using mutable state. */
static const uint16_t crc16_table[256] = {
#include "crc16_table.inc"
};
static const uint32_t crc32c_table[256] = {
#include "crc32c_table.inc"
};

uint16_t crc16_ccitt(const uint8_t *data, size_t length)
{
    uint16_t crc = 0xffff;
    while (length--)
        crc = (uint16_t)((crc << 8) ^ crc16_table[((crc >> 8) ^ *data++) & 0xff]);
    return crc;
}

uint32_t crc32c(const uint8_t *data, size_t length)
{
    uint32_t crc = ~0u;
    while (length--)
        crc = (crc >> 8) ^ crc32c_table[(crc ^ *data++) & 0xff];
    return ~crc;
}

/* Remap raw GPIO samples to the protocol's logical channel bit layout. The
 * board routes LA channels to scattered physical pins; PDBG_GPIO_MAP lists
 * each (physical, logical) pair so the host sees a stable bit map. */
uint32_t normalize_gpio(uint32_t raw)
{
    uint32_t sample = 0;
#define MAP_PIN(debug_pin, work_pin) sample |= ((raw >> debug_pin) & 1u) << work_pin;
    PDBG_GPIO_MAP(MAP_PIN)
#undef MAP_PIN
    return sample & PDBG_GPIO_CAPTURE_MASK;
}
