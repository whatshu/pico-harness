/* SPDX-License-Identifier: MIT */
#ifndef PICO_DUALCORE_CONFIG_H_
#define PICO_DUALCORE_CONFIG_H_

/* Board-specific configuration for pcb/dualcore. */
#define PROBE_SM 0
#define PROBE_PIN_OFFSET 14
#define PROBE_PIN_SWCLK 14
#define PROBE_PIN_SWDIO 15
#define PROBE_PIN_RESET 22
#define PROBE_RESET_ACTIVE_HIGH 1

/* PDBG channel 0 is Debug UART1, connected to Work UART0 GP12/GP13. */
#define PROBE_UART_COUNT 1
#define PROBE_UART0_TX 20
#define PROBE_UART0_RX 21
#define PROBE_UART0_INTERFACE uart1
#define PROBE_UART_BAUDRATE 115200
#define PROBE_UART_TX PROBE_UART0_TX
#define PROBE_UART_RX PROBE_UART0_RX
#define PROBE_UART_INTERFACE PROBE_UART0_INTERFACE
#define PROBE_USB_CONNECTED_LED 25

#define PDBG_WORK_ENABLE_PIN 0
#define PDBG_WORK_FAULT_PIN 1
#define PDBG_WORK_RUN_PIN 22
#define PDBG_WORK_BOOT_PIN 26
#define PDBG_WORK_ADC_PIN 28
#define PDBG_WORK_ADC_CHANNEL 2
#define PDBG_INA_I2C i2c0
#define PDBG_INA_SDA_PIN 4
#define PDBG_INA_SCL_PIN 5
#define PDBG_INA_ALERT_PIN 2

#define PDBG_SPI_INTERFACE spi1
#define PDBG_SPI_RX_PIN 8
#define PDBG_SPI_CS_PIN 9
#define PDBG_SPI_SCK_PIN 10
#define PDBG_SPI_TX_PIN 11

#define PDBG_GPIO_RAW_MASK 0x083f3fc8u
#define PDBG_GPIO_CAPTURE_MASK 0x1c3ffc20u
#define PDBG_GPIO_MAP(X) \
    X(3, 28)             \
    X(6, 27)             \
    X(7, 26)             \
    X(27, 5)             \
    X(12, 17)            \
    X(13, 16)            \
    X(18, 11)            \
    X(19, 10)            \
    X(16, 15)            \
    X(17, 14)            \
    X(20, 13)            \
    X(21, 12)            \
    X(8, 19)             \
    X(9, 21)             \
    X(10, 18)            \
    X(11, 20)

#if PICO_RP2040
#define PDBG_DEBUG_CORE "pico"
#define PROBE_PRODUCT_STRING "PicoHarness Pico (CMSIS-DAP)"
#else
#define PDBG_DEBUG_CORE "pico2"
#define PROBE_PRODUCT_STRING "PicoHarness Pico 2 (CMSIS-DAP)"
#endif
#define PDBG_INFO_STRING                                                     \
    "PicoHarness/4;dualcore;" PDBG_DEBUG_CORE                             \
    ";pdbg-link;dedicated-core1;uart-dma;spi-dma;la-gusmanb;capture-status;" \
    "debug-bootsel;single-uart;spi1;gpio-capture;capture-timing;capture-async;capture-pretrigger;run;enable;" \
    "bootsel;power-good;power-fault;ina237;measure-v2;power-default-off"

#endif
