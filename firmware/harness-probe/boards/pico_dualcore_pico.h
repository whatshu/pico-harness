/* SPDX-License-Identifier: BSD-3-Clause */
#ifndef _BOARDS_PICO_DUALCORE_PICO_H
#define _BOARDS_PICO_DUALCORE_PICO_H

pico_board_cmake_set(PICO_PLATFORM, rp2040)
#include "boards/pico.h"
/* TinyUSB board_init must not initialize a console on power-control GP0/1. */
#undef PICO_DEFAULT_UART
#undef PICO_DEFAULT_UART_TX_PIN
#undef PICO_DEFAULT_UART_RX_PIN

#endif
