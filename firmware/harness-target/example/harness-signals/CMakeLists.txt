# SPDX-License-Identifier: MIT
cmake_minimum_required(VERSION 3.13)
# Override with -DCMAKE_BUILD_TYPE=Release for production builds.
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Debug CACHE STRING "Build type")
endif()

# Use the Pico SDK's official import before project().
include($ENV{PICO_SDK_PATH}/external/pico_sdk_import.cmake)
project(harness_signals C CXX ASM)
pico_sdk_init()
add_subdirectory(../.. pico_harness)
add_executable(harness_signals main.c)
target_link_libraries(harness_signals PRIVATE pico_harness)
pico_enable_stdio_usb(harness_signals 0)
pico_enable_stdio_uart(harness_signals 0)
pico_add_extra_outputs(harness_signals)
