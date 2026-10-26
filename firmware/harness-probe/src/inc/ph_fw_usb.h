/* SPDX-License-Identifier: MIT */

#ifndef TUSB_HANDLER_H
#define TUSB_HANDLER_H

#include "FreeRTOS.h"
#include "task.h"

#include "tusb.h"

/*
 * TinyUSB device callbacks.  usbd.c discovers them by weak symbol name; the
 * prototypes below only expose them to other translation units (main.c calls
 * tud_unmount_cb() itself).
 */
void tud_event_hook_cb(uint8_t rhport, uint32_t eventid, bool in_isr);
bool tud_vendor_control_xfer_cb(uint8_t rhport, uint8_t stage,
                                tusb_control_request_t const *request);
void tud_suspend_cb(bool remote_wakeup_en);
void tud_resume_cb(void);
void tud_unmount_cb(void);
void tud_mount_cb(void);

/* True while the DAP endpoint group is configured (tud_mount_cb) and has not
 * been torn down by a bus reset / unmount (tud_unmount_cb). */
bool ph_fw_usb_dap_configured(void);

/* Defined in main.c; the TinyUSB event callbacks above use it to wake the
 * control task.  Exposed here so tusb_handler.c can see it. */
extern TaskHandle_t ph_fw_control_taskhandle;

#endif