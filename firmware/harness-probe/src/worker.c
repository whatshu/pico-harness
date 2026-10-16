/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/sync.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "logic_analyzer.h"
#include "ph_bulk.h"
#include "ph_terminal.h"
#include "target.h"
#include "worker.h"

static volatile bool debug_boot_requested;
static uint32_t debug_boot_deadline; /* owned by core 0 */

void ph_request_self_bootsel(void)
{
    debug_boot_requested = true;
}

void worker_init(void)
{
    ph_terminal_init();
    ph_bulk_init();

    /* The LA engine is kept armed for the SUMP phase; la_init claims PIO1. */
    la_init();
}

void worker_reset(void)
{
    ph_terminal_reset();
    ph_bulk_reset();
    debug_boot_requested = false;
    debug_boot_deadline = 0;
}

void worker_step(void)
{
    /* Core 0 owns board monitoring, power policy and the USB pumps. */
    target_control_poll();
    ph_bulk_pump_rx();
    ph_terminal_pump();

    if (debug_boot_requested) {
        /* Give the outstanding response time to leave the CDC before reset. */
        if (!debug_boot_deadline)
            debug_boot_deadline = time_us_32() + 100000;
        if ((int32_t)(time_us_32() - debug_boot_deadline) >= 0)
            reset_usb_boot(0, 0);
        return;
    }
}

void worker_thread(void)
{
    /* Core 1 has no RTOS tick, USB ISR, DMA ISR or FIFO doorbell ISR. */
    save_and_disable_interrupts();

    uint8_t console[64];
    for (;;) {
        target_serial_poll();

        /* Work UART output -> ConsoleData events (short messages). */
        for (int i = 0; i < 32; ++i) {
            size_t n = target_serial_read(0, console, sizeof(console));
            if (!n)
                break;
            ph_console_data_emit(console, n);
        }

        ph_terminal_worker();
        ph_bulk_worker();
        la_poll();
        tight_loop_contents();
    }
}