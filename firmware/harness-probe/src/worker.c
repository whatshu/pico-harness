/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/sync.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "logic_analyzer.h"
#include "ph_bulk.h"
#include "ph_console.h"
#include "ph_sump.h"
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
    ph_console_init();
    ph_sump_init();

    /* The LA engine is kept armed for the SUMP phase; la_init claims PIO1. */
    la_init();
}

void worker_reset(void)
{
    ph_terminal_reset();
    ph_bulk_reset();
    ph_console_reset();
    ph_sump_reset();
    debug_boot_requested = false;
    debug_boot_deadline = 0;
}

void worker_step(void)
{
    /* Core 0 owns board monitoring, power policy and the USB pumps. */
    target_control_poll();
    ph_bulk_pump_rx();
    ph_sump_pump();
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

    for (;;) {
        target_serial_poll();
        ph_console_pump(); /* Work console bridge (Hello / framing / baud) */

        ph_terminal_worker();
        ph_bulk_worker();
        ph_sump_worker();
        la_poll();
        tight_loop_contents();
    }
}