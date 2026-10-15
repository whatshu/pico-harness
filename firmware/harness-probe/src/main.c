/* SPDX-License-Identifier: MIT */
/*
 * Copyright (c) 2021 Raspberry Pi (Trading) Ltd.
 * Copyright (c) 2021 Peter Lawrence
 */

#include "hardware/structs/usb.h"
#include "pico/binary_info.h"
#include "pico/multicore.h"
#include "pico/stdlib.h"
#include "probe/version.h"

#include "bsp/board_api.h"
#include "tusb.h"

#include "DAP.h"
#include "FreeRTOS.h"
#include "target.h"
#include "task.h"
#include "tusb_handler.h"
#include "util.h"
#include "worker.h"

#define THREADED 1

#define CONTROL_TASK_PRIO (tskIDLE_PRIORITY + 2)

TaskHandle_t control_taskhandle;
#if PICO_RP2040
static TaskHandle_t usb_watchdog_taskhandle;
#endif

void bi_decl_config(void)
{
    bi_decl(bi_program_version_string(PROBE_VERSION));
    bi_decl(bi_program_feature(PDBG_INFO_STRING));
    bi_decl(bi_1pin_with_name(PROBE_PIN_RESET, "PROBE RESET"));
    bi_decl(bi_1pin_with_name(PROBE_PIN_SWCLK, "PROBE SWCLK"));
    bi_decl(bi_1pin_with_name(PROBE_PIN_SWDIO, "PROBE SWDIO"));
    bi_decl(bi_program_feature("PROBE UART INTERFACE " PDBG_STR(PROBE_UART_INTERFACE)));
    bi_decl(bi_program_feature("PROBE UART BAUDRATE " PDBG_STR(PROBE_UART_BAUDRATE)));
    bi_decl(bi_1pin_with_name(PROBE_UART_TX, "PROBE UART TX"));
    bi_decl(bi_1pin_with_name(PROBE_UART_RX, "PROBE UART RX"));
}

void usb_watchdog_thread(void *ptr)
{
    uint32_t sof[3];
    int i = 0;
    TickType_t wake;
    wake = xTaskGetTickCount();

    do {
        /* ~5 SOF events per tick */
        xTaskDelayUntil(&wake, 100);
        if (tud_connected() && !tud_suspended()) {
            sof[i++] = usb_hw->sof_rd & USB_SOF_RD_BITS;
            i = i % 3;
        } else {
            for (i = 0; i < 3; i++)
                sof[i] = 0;
        }

        if ((sof[0] | sof[1] | sof[2]) != 0) {
            if ((sof[0] == sof[1]) && (sof[1] == sof[2])) {
                probe_info("Watchdog timeout! Resetting USBD\n");
                /* uh oh, signal disconnect (implicitly resets the controller) */
                tud_deinit(0);
                /* Make sure the port got the message */
                xTaskDelayUntil(&wake, 1);
                tud_init(0);
            }
        }
    } while (1);
}

/*
 * Run one USB update cycle: feed TinyUSB, detect an implied bus reset (a
 * configured connection going away while we run), then block until there is
 * work again.  While suspended / disconnected the loop keeps a 1 ms cadence
 * for the SOF watchdog; otherwise it sleeps on the task notification sent
 * from tud_event_hook_cb() in tusb_handler.c.
 */
static void tud_update(TickType_t *wake)
{
    tud_task();

    /* implied bus-reset detection */
    if (!tud_connected() && tusb_dap_configured())
        tud_unmount_cb();

    /* If suspended or disconnected, unconditional delay for 1ms (20 ticks) */
    if (tud_suspended() || !tud_connected())
        xTaskDelayUntil(wake, 20);
    /* Go to sleep if nothing to do */
    else if (!tud_task_event_ready())
        xTaskNotifyWait(0, 0xFFFFFFFFu, NULL, 1);
}

void control_thread(void *ptr)
{
    TickType_t wake;

    target_led_init();
    wake = xTaskGetTickCount();

    do {
        tud_update(&wake);
        worker_step();
        target_led_tick();
    } while (1);
}

int main(void)
{
    /* Declare pins in binary information */
    bi_decl_config();

    target_io_init();
    board_init();
    target_serial_init();
    worker_init();
    tusb_init();
#if LIB_PICO_STDIO_UART
    stdio_uart_init();
#endif

    DAP_Setup();
    multicore_launch_core1(worker_thread);

    probe_info("Welcome to PicoHarness!\n");

    if (THREADED) {
        xTaskCreate(control_thread, "CTRL", configMINIMAL_STACK_SIZE, NULL, CONTROL_TASK_PRIO,
                    &control_taskhandle);
#if PICO_RP2040
        xTaskCreate(usb_watchdog_thread, "WDOG", configMINIMAL_STACK_SIZE, NULL, CONTROL_TASK_PRIO,
                    &usb_watchdog_taskhandle);
#endif
        vTaskStartScheduler();
    }
    return 0;
}

void vApplicationTickHook(void)
{
}

void vApplicationStackOverflowHook(TaskHandle_t task, char *pcTaskName)
{
    panic("stack overflow (not the helpful kind) for %s\n", pcTaskName);
}

void vApplicationMallocFailedHook(void)
{
    panic("Malloc Failed\n");
}
