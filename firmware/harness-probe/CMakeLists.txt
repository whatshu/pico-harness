cmake_minimum_required(VERSION 3.12)

set(CMAKE_BUILD_TYPE RelWithDebInfo)

get_filename_component(PROJECT_ROOT "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
if (NOT PICO_SDK_PATH)
    if (DEFINED ENV{PICO_SDK_PATH})
        set(PICO_SDK_PATH "$ENV{PICO_SDK_PATH}" CACHE PATH "Pico SDK path")
    else()
        message(FATAL_ERROR "PICO_SDK_PATH must point to the installed Pico SDK")
    endif()
endif()
set(PICO_BOARD pico_dualcore CACHE STRING "dualcore Debug core board")
list(APPEND PICO_BOARD_HEADER_DIRS "${CMAKE_CURRENT_LIST_DIR}/boards")
include(${PICO_SDK_PATH}/external/pico_sdk_import.cmake)

set(FREERTOS_KERNEL_PATH "${PROJECT_ROOT}/third_party/FreeRTOS-Kernel")
include(FreeRTOS_Kernel_import.cmake)

project(harness-probe)

pico_sdk_init()

if (${PICO_SDK_VERSION_MAJOR} LESS 2)
	message(SEND_ERROR "Version 2 of the Pico SDK is required to compile this project. Please update your installation at ${PICO_SDK_PATH}")
endif ()

add_executable(harness-probe
        src/main.c
        src/swd.c
        src/tusb_handler.c
        src/worker.c
        src/util.c
        src/target_io.c
        src/target_control.c
        src/ph_terminal.c
        src/ph_console.c
        src/ph_bulk.c
        src/ph.pb.c
)

target_sources(harness-probe PRIVATE
        ${PROJECT_ROOT}/third_party/cmsis-dap/CMSIS/DAP/Firmware/Source/DAP.c
        ${PROJECT_ROOT}/third_party/cmsis-dap/CMSIS/DAP/Firmware/Source/JTAG_DP.c
        ${PROJECT_ROOT}/third_party/cmsis-dap/CMSIS/DAP/Firmware/Source/DAP_vendor.c
        ${PROJECT_ROOT}/third_party/cmsis-dap/CMSIS/DAP/Firmware/Source/SWO.c
        ${PROJECT_ROOT}/third_party/nanopb/pb_common.c
        ${PROJECT_ROOT}/third_party/nanopb/pb_encode.c
        ${PROJECT_ROOT}/third_party/nanopb/pb_decode.c
        )

target_include_directories(harness-probe PRIVATE
        ${PROJECT_ROOT}/third_party/cmsis-dap/CMSIS/DAP/Firmware/Include/
        ${PROJECT_ROOT}/third_party/cmsis-dap/CMSIS/Core/Include/
        ${PROJECT_ROOT}/third_party/nanopb/
        include/
        boards/
        )

target_compile_options(harness-probe PRIVATE -Wall)

pico_generate_pio_header(harness-probe ${CMAKE_CURRENT_LIST_DIR}/src/pio/probe.pio)

target_include_directories(harness-probe PRIVATE src/inc)
target_include_directories(harness-probe PRIVATE src)

# add version
add_custom_target(version
        ${CMAKE_SOURCE_DIR}/get-version.sh
        WORKING_DIRECTORY ${CMAKE_BINARY_DIR}
)

target_include_directories(harness-probe PRIVATE
        ${CMAKE_BINARY_DIR}/generated
        )

add_dependencies(harness-probe version)

target_compile_definitions (harness-probe PRIVATE
	PICO_RP2040_USB_DEVICE_ENUMERATION_FIX=1
)

if (PICO_BOARD STREQUAL "pico_dualcore" OR PICO_BOARD STREQUAL "pico_dualcore_pico")
    if (PICO_BOARD STREQUAL "pico_dualcore_pico")
        set(PDBG_DEBUG_CORE "pico")
    else ()
        set(PDBG_DEBUG_CORE "pico2")
    endif ()
    set_target_properties(harness-probe PROPERTIES OUTPUT_NAME "pico_dualcore_debug_${PDBG_DEBUG_CORE}")
    pico_set_program_name(harness-probe "PicoHarness Debug (${PDBG_DEBUG_CORE})")
    target_link_libraries(harness-probe PRIVATE hardware_adc)
    target_sources(harness-probe PRIVATE src/logic_analyzer.c)
    target_compile_definitions(harness-probe PRIVATE PICO_CORE1_STACK_SIZE=4096)
    pico_generate_pio_header(harness-probe ${CMAKE_CURRENT_LIST_DIR}/src/pio/logic_analyzer.pio)
else ()
    message(FATAL_ERROR "Unsupported board ${PICO_BOARD}; use pico_dualcore or pico_dualcore_pico")
endif ()


target_link_libraries(harness-probe PRIVATE
        pico_multicore
        pico_stdlib
        pico_unique_id
        tinyusb_device
        tinyusb_board
        hardware_pio
        hardware_dma
        hardware_irq
        hardware_clocks
        hardware_i2c
        hardware_spi
        FreeRTOS-Kernel
        FreeRTOS-Kernel-Heap1
)

if (PICO_BOARD STREQUAL "pico_dualcore_pico")
    # Keep the full 32768-sample capture buffer and RTOS heap within RP2040's
    # 264 KiB SRAM. PIO/DMA capture runs independently of flash execution.
    pico_set_binary_type(harness-probe default)
else ()
    pico_set_binary_type(harness-probe copy_to_ram)
endif ()

pico_enable_stdio_uart(harness-probe 0)
pico_enable_stdio_usb(harness-probe 0)

pico_add_extra_outputs(harness-probe)
