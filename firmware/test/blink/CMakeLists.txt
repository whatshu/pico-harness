# SPDX-License-Identifier: MIT
cmake_minimum_required(VERSION 3.13)
# Override with -DCMAKE_BUILD_TYPE=Release for production builds.
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Debug CACHE STRING "Build type")
endif()

# Use the Pico SDK's official import before project().
include($ENV{PICO_SDK_PATH}/external/pico_sdk_import.cmake)
project(blink C CXX ASM)
pico_sdk_init()
add_executable(blink main.c)
target_link_libraries(blink PRIVATE pico_stdlib hardware_uart)
pico_enable_stdio_usb(blink 0)
pico_enable_stdio_uart(blink 0)
pico_add_extra_outputs(blink)
