# SPDX-License-Identifier: MIT
cmake_minimum_required(VERSION 3.13)
# Override with -DCMAKE_BUILD_TYPE=Release for production builds.
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Debug CACHE STRING "Build type")
endif()

# Use the Pico SDK's official import before project().
include($ENV{PICO_SDK_PATH}/external/pico_sdk_import.cmake)
project(uart_console C CXX ASM)
pico_sdk_init()
add_executable(uart_console main.c)
target_link_libraries(uart_console PRIVATE pico_stdlib hardware_uart)
pico_enable_stdio_usb(uart_console 0)
pico_enable_stdio_uart(uart_console 0)
pico_add_extra_outputs(uart_console)
