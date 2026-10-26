# SPDX-License-Identifier: MIT
add_library(pico_harness STATIC src/pico_harness.c src/spi_link.c)
target_include_directories(pico_harness PUBLIC include PRIVATE src)
target_link_libraries(pico_harness PUBLIC pico_stdlib hardware_uart hardware_pwm hardware_pio hardware_dma)
pico_generate_pio_header(pico_harness ${CMAKE_CURRENT_LIST_DIR}/src/spi_slave.pio)
target_compile_options(pico_harness PRIVATE -Wall -Wextra)
