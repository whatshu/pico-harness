/* SPDX-License-Identifier: MIT AND BSD-3-Clause */
/*
 * USB device layer for the debug probe: TinyUSB descriptors, device event
 * callbacks, and the CMSIS-DAP vendor class driver.
 *
 * Copyright (c) 2019 Ha Thach (tinyusb.org)
 * Copyright (c) 2021 Raspberry Pi (Trading) Ltd.
 * Copyright (c) 2021 Peter Lawrence
 * Copyright (c) 2022 Raspberry Pi Ltd
 * Copyright (c) 2023 Raspberry Pi (Trading) Ltd.
 */

#include <string.h>

#include "device/usbd_pvt.h"
#include "tusb.h"

#include "DAP.h"
#include "DAP_config.h"
#include "FreeRTOS.h"
#include "semphr.h"
#include "ph_fw_target.h"
#include "task.h"
#include "ph_fw_usb.h"
#include "ph_fw_worker.h"

#define DAP_TASK_PRIO (tskIDLE_PRIORITY + 1)

#define DAP_OUT_EP_NUM 0x04
#define DAP_IN_EP_NUM 0x85
#define PH_FW_CDC0_NOTIFY_EP_NUM 0x82
#define PH_FW_CDC0_OUT_EP_NUM 0x02
#define PH_FW_CDC0_IN_EP_NUM 0x83
#define PH_FW_BULK_OUT_EP_NUM 0x05
#define PH_FW_BULK_IN_EP_NUM 0x86
#define PH_FW_CDC1_NOTIFY_EP_NUM 0x81
#define PH_FW_CDC1_OUT_EP_NUM 0x06
#define PH_FW_CDC1_IN_EP_NUM 0x87

/* PH Terminal Bulk is a generic vendor interface (FF/50/01), distinct from the
 * CMSIS-DAP driver's FF/00/00 so the built-in TinyUSB vendor class claims it. */
#define PH_FW_TUD_PH_BULK_DESCRIPTOR(_itfnum, _stridx, _epout, _epin, _epsize)                        \
    9, TUSB_DESC_INTERFACE, _itfnum, 0, 2, TUSB_CLASS_VENDOR_SPECIFIC, 0x50, 0x01, _stridx, 7,  \
        TUSB_DESC_ENDPOINT, _epout, TUSB_XFER_BULK, U16_TO_U8S_LE(_epsize), 0, 7,               \
        TUSB_DESC_ENDPOINT, _epin, TUSB_XFER_BULK, U16_TO_U8S_LE(_epsize), 0

#define PH_FW_CONFIG_TOTAL_LEN (TUD_CONFIG_DESC_LEN + TUD_VENDOR_DESC_LEN + TUD_CDC_DESC_LEN + \
                          TUD_VENDOR_DESC_LEN + TUD_CDC_DESC_LEN)
#define PH_FW_MS_OS_20_DESC_LEN 0xB2
#define PH_FW_BOS_TOTAL_LEN (TUD_BOS_DESC_LEN + TUD_BOS_MICROSOFT_OS_DESC_LEN)

#define DAP_INTERFACE_SUBCLASS 0x00
#define DAP_INTERFACE_PROTOCOL 0x00
#define PH_FW_WR_IDX(x) (x.write_idx % DAP_PACKET_COUNT)
#define PH_FW_RD_IDX(x) (x.read_idx % DAP_PACKET_COUNT)
#define PH_FW_WR_SLOT_PTR(x) &(x.data[PH_FW_WR_IDX(x)][0])
#define PH_FW_RD_SLOT_PTR(x) &(x.data[PH_FW_RD_IDX(x)][0])

enum {
    ITF_DAP, /* Old versions of Keil MDK only look at interface 0 */
    ITF_CDC0,
    ITF_CDC0_DATA,
    ITF_BULK,
    ITF_CDC1,
    ITF_CDC1_DATA,
    ITF_TOTAL
};

typedef struct {
    uint8_t data[DAP_PACKET_COUNT][DAP_PACKET_SIZE];
    uint16_t data_len[DAP_PACKET_COUNT];
    volatile uint32_t write_idx;
    volatile uint32_t read_idx;
    volatile bool was_empty;
    volatile bool was_full;
} dap_buffer_t;

/* Owned here because tud_mount_cb() / tud_unmount_cb() decide its lifetime. */
static TaskHandle_t dap_taskhandle;
static int was_configured;
static uint8_t itf_num, dap_rhport;
static uint8_t out_ep_addr, in_ep_addr;
static dap_buffer_t usb_request_buffer, usb_response_buffer;
static SemaphoreHandle_t edpt_lock;

static char *dap_cmd_string[] = {
    [ID_DAP_Info] = "DAP_Info",
    [ID_DAP_HostStatus] = "DAP_HostStatus",
    [ID_DAP_Connect] = "DAP_Connect",
    [ID_DAP_Disconnect] = "DAP_Disconnect",
    [ID_DAP_TransferConfigure] = "DAP_TransferConfigure",
    [ID_DAP_Transfer] = "DAP_Transfer",
    [ID_DAP_TransferBlock] = "DAP_TransferBlock",
    [ID_DAP_TransferAbort] = "DAP_TransferAbort",
    [ID_DAP_WriteABORT] = "DAP_WriteABORT",
    [ID_DAP_Delay] = "DAP_Delay",
    [ID_DAP_ResetTarget] = "DAP_ResetTarget",
    [ID_DAP_SWJ_Pins] = "DAP_SWJ_Pins",
    [ID_DAP_SWJ_Clock] = "DAP_SWJ_Clock",
    [ID_DAP_SWJ_Sequence] = "DAP_SWJ_Sequence",
    [ID_DAP_SWD_Configure] = "DAP_SWD_Configure",
    [ID_DAP_SWD_Sequence] = "DAP_SWD_Sequence",
    [ID_DAP_JTAG_Sequence] = "DAP_JTAG_Sequence",
    [ID_DAP_JTAG_Configure] = "DAP_JTAG_Configure",
    [ID_DAP_JTAG_IDCODE] = "DAP_JTAG_IDCODE",
    [ID_DAP_SWO_Transport] = "DAP_SWO_Transport",
    [ID_DAP_SWO_Mode] = "DAP_SWO_Mode",
    [ID_DAP_SWO_Baudrate] = "DAP_SWO_Baudrate",
    [ID_DAP_SWO_Control] = "DAP_SWO_Control",
    [ID_DAP_SWO_Status] = "DAP_SWO_Status",
    [ID_DAP_SWO_ExtendedStatus] = "DAP_SWO_ExtendedStatus",
    [ID_DAP_SWO_Data] = "DAP_SWO_Data",
    [ID_DAP_QueueCommands] = "DAP_QueueCommands",
    [ID_DAP_ExecuteCommands] = "DAP_ExecuteCommands",
};

/* Main DAP loop, defined below; tud_mount_cb() spawns it via xTaskCreate(). */
static void ph_fw_dap_thread(void *ptr);
static void dap_edpt_init(void);
static bool dap_edpt_deinit(void);
static void dap_edpt_reset(uint8_t __unused rhport);
static uint16_t dap_edpt_open(uint8_t __unused rhport, tusb_desc_interface_t const *itf_desc,
                              uint16_t max_len);
static bool dap_edpt_control_xfer_cb(uint8_t __unused rhport, uint8_t stage,
                                     tusb_control_request_t const *request);
static bool dap_edpt_xfer_cb(uint8_t __unused rhport, uint8_t ep_addr, xfer_result_t result,
                             uint32_t xferred_bytes);

/* USB descriptors and class state live before the callback implementations. */

/* Device descriptor */
tusb_desc_device_t const desc_device = {
    .bLength = sizeof(tusb_desc_device_t),
    .bDescriptorType = TUSB_DESC_DEVICE,
    .bcdUSB = 0x0210, /* USB Specification version 2.1 for BOS */
    .bDeviceClass = 0x00, /* Each interface specifies its own */
    .bDeviceSubClass = 0x00, /* Each interface specifies its own */
    .bDeviceProtocol = 0x00,
    .bMaxPacketSize0 = CFG_TUD_ENDPOINT0_SIZE,

    .idVendor = 0x2E8A, /* Pi */
    .idProduct = 0x000c, /* CMSIS-DAP Debug Probe */
    .bcdDevice = 0x0231, /* Version 02.31 */
    .iManufacturer = 0x01,
    .iProduct = 0x02,
    .iSerialNumber = 0x03,
    .bNumConfigurations = 0x01
};

/* Configuration descriptor */
uint8_t const desc_configuration[] = {
    TUD_CONFIG_DESCRIPTOR(1, ITF_TOTAL, 0, PH_FW_CONFIG_TOTAL_LEN, 0, 100),
    /* Interface 0: CMSIS-DAP v2 (SWD). Keil MDK expects interface 0. */
    TUD_VENDOR_DESCRIPTOR(ITF_DAP, 4, DAP_OUT_EP_NUM, DAP_IN_EP_NUM, 64),
    /* Interface 1,2 (CDC): PH Terminal -- all control/console messages. */
    TUD_CDC_DESCRIPTOR(ITF_CDC0, 5, PH_FW_CDC0_NOTIFY_EP_NUM, 8, PH_FW_CDC0_OUT_EP_NUM, PH_FW_CDC0_IN_EP_NUM, 64),
    /* Interface 3 (vendor, FF/50/01): PH Terminal Bulk -- raw payload data plane. */
    PH_FW_TUD_PH_BULK_DESCRIPTOR(ITF_BULK, 6, PH_FW_BULK_OUT_EP_NUM, PH_FW_BULK_IN_EP_NUM, 64),
    /* Interface 4,5 (CDC): Logic Analyzer (SUMP) -- reserved for the SUMP phase. */
    TUD_CDC_DESCRIPTOR(ITF_CDC1, 7, PH_FW_CDC1_NOTIFY_EP_NUM, 8, PH_FW_CDC1_OUT_EP_NUM, PH_FW_CDC1_IN_EP_NUM, 64),
};

TU_VERIFY_STATIC(sizeof(desc_configuration) == PH_FW_CONFIG_TOTAL_LEN,
                 "Incorrect configuration descriptor size");

/* String descriptors */
/* array of pointer to string descriptors */
char const *string_desc_arr[] = {
    (const char[]){ 0x09, 0x04 }, /* 0: is supported language is English (0x0409) */
    "Raspberry Pi", /* 1: Manufacturer */
    PROBE_PRODUCT_STRING, /* 2: Product */
    ph_fw_usb_serial, /* 3: Serial, uses flash unique ID */
    "CMSIS-DAP v2 Interface", /* 4: Interface descriptor for Bulk transport */
    "PH Terminal", /* 5: PH protocol channel (control/console/coordination) */
    "PH Terminal Bulk", /* 6: raw payload data plane */
    "Pico Harness Logic Analyzer", /* 7: SUMP logic analyzer */
};

static uint16_t _desc_str[32];

/* BOS descriptor (Microsoft OS 2.0) */
/*
 * Microsoft OS 2.0 registry property descriptor.
 * Per MS requirements https://msdn.microsoft.com/en-us/library/windows/hardware/hh450799(v=vs.85).aspx
 * device should create DeviceInterfaceGUIDs. It can be done by driver and in
 * case of real PnP solution, device should expose MS "Microsoft OS 2.0
 * registry property descriptor". Such descriptor can insert any record into
 * Windows registry per device / configuration / interface. In our case it
 * will insert "DeviceInterfaceGUIDs" multistring property.
 *
 * https://developers.google.com/web/fundamentals/native-hardware/build-for-webusb/
 * (Section Microsoft OS compatibility descriptors)
 */
uint8_t const desc_bos[] = { /* total length, number of device caps */
                             TUD_BOS_DESCRIPTOR(PH_FW_BOS_TOTAL_LEN, 1),

                             /* Microsoft OS 2.0 descriptor */
                             TUD_BOS_MS_OS_20_DESCRIPTOR(PH_FW_MS_OS_20_DESC_LEN, 1)
};

uint8_t const desc_ms_os_20[] = {
    /* Set header: length, type, windows version, total length */
    U16_TO_U8S_LE(0x000A), U16_TO_U8S_LE(MS_OS_20_SET_HEADER_DESCRIPTOR), U32_TO_U8S_LE(0x06030000),
    U16_TO_U8S_LE(PH_FW_MS_OS_20_DESC_LEN),

    /* Configuration subset header: length, type, configuration index, reserved,
     * configuration total length */
    U16_TO_U8S_LE(0x0008), U16_TO_U8S_LE(MS_OS_20_SUBSET_HEADER_CONFIGURATION), 0, 0,
    U16_TO_U8S_LE(PH_FW_MS_OS_20_DESC_LEN - 0x0A),

    /* Function Subset header: length, type, first interface, reserved, subset length */
    U16_TO_U8S_LE(0x0008), U16_TO_U8S_LE(MS_OS_20_SUBSET_HEADER_FUNCTION), ITF_DAP, 0,
    U16_TO_U8S_LE(PH_FW_MS_OS_20_DESC_LEN - 0x0A - 0x08),

    /* MS OS 2.0 Compatible ID descriptor: length, type, compatible ID, sub compatible ID */
    U16_TO_U8S_LE(0x0014), U16_TO_U8S_LE(MS_OS_20_FEATURE_COMPATBLE_ID), 'W', 'I', 'N', 'U', 'S',
    'B', 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, /* sub-compatible */

    /* MS OS 2.0 Registry property descriptor: length, type */
    U16_TO_U8S_LE(PH_FW_MS_OS_20_DESC_LEN - 0x0A - 0x08 - 0x08 - 0x14),
    U16_TO_U8S_LE(MS_OS_20_FEATURE_REG_PROPERTY), U16_TO_U8S_LE(0x0007),
    /* wPropertyDataType, wPropertyNameLength and the property name
     * string "DeviceInterfaceGUIDs", all encoded as UTF-16. */
    U16_TO_U8S_LE(0x002A), 'D', 0x00, 'e', 0x00, 'v', 0x00, 'i', 0x00, 'c', 0x00, 'e', 0x00, 'I',
    0x00, 'n', 0x00, 't', 0x00, 'e', 0x00, 'r', 0x00, 'f', 0x00, 'a', 0x00, 'c', 0x00, 'e', 0x00,
    'G', 0x00, 'U', 0x00, 'I', 0x00, 'D', 0x00, 's', 0x00, 0x00, 0x00,
    U16_TO_U8S_LE(0x0050), /* wPropertyDataLength */
    /* bPropertyData "{CDB3B5AD-293B-4663-AA36-1AAE46463776}" as a UTF-16
     * string (b doesn't mean bytes) */
    '{', 0x00, 'C', 0x00, 'D', 0x00, 'B', 0x00, '3', 0x00, 'B', 0x00, '5', 0x00, 'A', 0x00, 'D',
    0x00, '-', 0x00, '2', 0x00, '9', 0x00, '3', 0x00, 'B', 0x00, '-', 0x00, '4', 0x00, '6', 0x00,
    '6', 0x00, '3', 0x00, '-', 0x00, 'A', 0x00, 'A', 0x00, '3', 0x00, '6', 0x00, '-', 0x00, '1',
    0x00, 'A', 0x00, 'A', 0x00, 'E', 0x00, '4', 0x00, '6', 0x00, '4', 0x00, '6', 0x00, '3', 0x00,
    '7', 0x00, '7', 0x00, '6', 0x00, '}', 0x00, 0x00, 0x00, 0x00, 0x00
};

TU_VERIFY_STATIC(sizeof(desc_ms_os_20) == PH_FW_MS_OS_20_DESC_LEN, "Incorrect size");

static usbd_class_driver_t const dap_edpt_driver = { .init = dap_edpt_init,
                                                     .deinit = dap_edpt_deinit,
                                                     .reset = dap_edpt_reset,
                                                     .open = dap_edpt_open,
                                                     .control_xfer_cb = dap_edpt_control_xfer_cb,
                                                     .xfer_cb = dap_edpt_xfer_cb,
                                                     .sof = NULL,
#if CFG_TUSB_DEBUG >= 2
                                                     .name = "DAP ENDPOINT"
#endif
};

/* Descriptors must remain valid for the duration of a control transfer. */
uint8_t const *tud_descriptor_device_cb(void)
{
    return (uint8_t const *)&desc_device;
}

uint8_t const *tud_descriptor_configuration_cb(uint8_t index)
{
    (void)index;
    return desc_configuration;
}

uint16_t const *tud_descriptor_string_cb(uint8_t index, uint16_t langid)
{
    (void)langid;

    uint8_t chr_count;

    if (index == 0) {
        memcpy(&_desc_str[1], string_desc_arr[0], 2);
        chr_count = 1;
    } else {
        /* Convert ASCII string into UTF-16. */
        if (!(index < sizeof(string_desc_arr) / sizeof(string_desc_arr[0])))
            return NULL;

        const char *str = string_desc_arr[index];
        chr_count = strlen(str);
        if (chr_count > 31)
            chr_count = 31;

        for (uint8_t i = 0; i < chr_count; i++)
            _desc_str[1 + i] = str[i];
    }

    /* First byte is the length, second byte is the string type. */
    _desc_str[0] = (TUSB_DESC_STRING << 8) | (2 * chr_count + 2);

    return _desc_str;
}

uint8_t const *tud_descriptor_bos_cb(void)
{
    return desc_bos;
}

bool ph_fw_usb_dap_configured(void)
{
    return was_configured;
}

/* --------------------------------------------------------------------- */
/* Device event callbacks                                                */
/* --------------------------------------------------------------------- */

void tud_event_hook_cb(uint8_t rhport, uint32_t eventid, bool in_isr)
{
    (void)rhport;
    (void)eventid;
    BaseType_t woken;
    if (in_isr) {
        xTaskNotifyFromISR(ph_fw_control_taskhandle, 0, 0, &woken);
    } else {
        xTaskNotify(ph_fw_control_taskhandle, 0, 0);
    }
}

bool tud_vendor_control_xfer_cb(uint8_t rhport, uint8_t stage,
                                tusb_control_request_t const *request)
{
    /* nothing to with DATA & ACK stage */
    if (stage != CONTROL_STAGE_SETUP)
        return true;

    switch (request->bmRequestType_bit.type) {
    case TUSB_REQ_TYPE_VENDOR:
        switch (request->bRequest) {
        case 1:
            if (request->wIndex == 7) {
                /* Get Microsoft OS 2.0 compatible descriptor */
                uint16_t total_len;
                memcpy(&total_len, desc_ms_os_20 + 8, 2);

                return tud_control_xfer(rhport, request, (void *)desc_ms_os_20, total_len);
            } else {
                return false;
            }

        default:
            break;
        }
        break;
    default:
        break;
    }

    /* stall unknown request */
    return false;
}

void tud_suspend_cb(bool remote_wakeup_en)
{
    ph_fw_target_led_set_mode(PH_FW_LED_OFF);
    ph_fw_probe_info("Suspended\n");
    /* Were we actually configured? If not, threads don't exist */
    if (was_configured) {
        vTaskSuspend(dap_taskhandle);
    }
    /* slow down clk_sys for power saving ? */
}

void tud_resume_cb(void)
{
    ph_fw_target_led_set_mode(PH_FW_LED_IDLE);
    ph_fw_probe_info("Resumed\n");
    if (was_configured) {
        vTaskResume(dap_taskhandle);
    }
}

void tud_unmount_cb(void)
{
    ph_fw_target_led_set_mode(PH_FW_LED_OFF);
    ph_fw_worker_reset();
    if (!was_configured)
        return;
    ph_fw_probe_info("Disconnected/reset\n");
    vTaskSuspend(dap_taskhandle);
    vTaskDelete(dap_taskhandle);
    was_configured = 0;
}

void tud_mount_cb(void)
{
    ph_fw_target_led_set_mode(PH_FW_LED_IDLE);
    ph_fw_probe_info("Connected, Configured: %d\n", !!was_configured);
    if (!was_configured) {
        /* Lowest priority thread is debug - need to shuffle buffers before we can toggle swd... */
        xTaskCreate(ph_fw_dap_thread, "DAP", configMINIMAL_STACK_SIZE, NULL, DAP_TASK_PRIO,
                    &dap_taskhandle);
        was_configured = 1;
    }
}

/* --------------------------------------------------------------------- */
/* CMSIS-DAP class driver                                                */
/* --------------------------------------------------------------------- */

static bool buffer_full(dap_buffer_t *buffer)
{
    return (buffer->write_idx + 1) % DAP_PACKET_COUNT == buffer->read_idx % DAP_PACKET_COUNT;
}

static bool buffer_empty(dap_buffer_t *buffer)
{
    return buffer->write_idx == buffer->read_idx;
}

/* Defer setup to .reset() / .open() */
static void dap_edpt_init(void)
{
    edpt_lock = xSemaphoreCreateMutex();
    xSemaphoreGive(edpt_lock);
}

/* This only gets called if the SOF watchdog times out */
static bool dap_edpt_deinit(void)
{
    if (edpt_lock != NULL)
        vSemaphoreDelete(edpt_lock);
    edpt_lock = NULL;
    return true;
}

static void dap_edpt_reset(uint8_t __unused rhport)
{
    ph_fw_probe_info("dap_edpt_reset\n");
    memset(&usb_request_buffer, 0, sizeof(usb_request_buffer));
    memset(&usb_response_buffer, 0, sizeof(usb_response_buffer));

    /* Reset ring positions and the flags used by endpoint callbacks. */
    usb_response_buffer.write_idx = 0;
    usb_response_buffer.read_idx = 0;
    usb_request_buffer.write_idx = 0;
    usb_request_buffer.read_idx = 0;

    usb_response_buffer.was_full = false;
    usb_response_buffer.was_empty = true;
    usb_request_buffer.was_full = false;
    usb_request_buffer.was_empty = true;
    /* Linux can reset the interface twice in succession. */
    itf_num = 0;
}

static uint16_t dap_edpt_open(uint8_t __unused rhport, tusb_desc_interface_t const *itf_desc,
                              uint16_t max_len)
{
    /* Ignore interfaces that do not belong to the CMSIS-DAP class driver. */
    TU_VERIFY(TUSB_CLASS_VENDOR_SPECIFIC == itf_desc->bInterfaceClass &&
                  DAP_INTERFACE_SUBCLASS == itf_desc->bInterfaceSubClass &&
                  DAP_INTERFACE_PROTOCOL == itf_desc->bInterfaceProtocol,
              0);

    uint16_t const drv_len =
        sizeof(tusb_desc_interface_t) + (itf_desc->bNumEndpoints * sizeof(tusb_desc_endpoint_t));
    TU_VERIFY(max_len >= drv_len, 0);
    itf_num = itf_desc->bInterfaceNumber;
    dap_rhport = rhport;

    /* Arm OUT immediately so the host can send the first command. */
    tusb_desc_endpoint_t *edpt_desc = (tusb_desc_endpoint_t *)(itf_desc + 1);
    uint8_t ep_addr = edpt_desc->bEndpointAddress;

    out_ep_addr = ep_addr;

    TU_ASSERT(usbd_edpt_open(rhport, edpt_desc), 0);
    TU_ASSERT(usbd_edpt_xfer(rhport, ep_addr, PH_FW_WR_SLOT_PTR(usb_request_buffer), DAP_PACKET_SIZE), 0);

    /* IN is queued by ph_fw_dap_thread once a response exists. */
    edpt_desc++;
    ep_addr = edpt_desc->bEndpointAddress;

    in_ep_addr = ep_addr;

    TU_ASSERT(usbd_edpt_open(rhport, edpt_desc), 0);

    return drv_len;
}

static bool dap_edpt_control_xfer_cb(uint8_t __unused rhport, uint8_t stage,
                                     tusb_control_request_t const *request)
{
    return false;
}

/* Advance each endpoint's ring position after TinyUSB completes a transfer. */
static bool dap_edpt_xfer_cb(uint8_t __unused rhport, uint8_t ep_addr, xfer_result_t result,
                             uint32_t xferred_bytes)
{
    const uint8_t ep_dir = tu_edpt_dir(ep_addr);

    if (ep_dir == TUSB_DIR_IN) {
        if (xferred_bytes >= 0u && xferred_bytes <= DAP_PACKET_SIZE) {
            xSemaphoreTake(edpt_lock, portMAX_DELAY);
            usb_response_buffer.read_idx++;
            /* The worker may have queued several responses while USB was busy.
             * Keep sending until the next completion reaches its write index. */
            if (!usb_response_buffer.was_empty) {
                usbd_edpt_xfer(rhport, ep_addr, PH_FW_RD_SLOT_PTR(usb_response_buffer),
                               usb_response_buffer.data_len[PH_FW_RD_IDX(usb_response_buffer)]);
                usb_response_buffer.was_empty = (usb_response_buffer.read_idx + 1) ==
                                                usb_response_buffer.write_idx;
            }
            xSemaphoreGive(edpt_lock);

            xTaskNotify(dap_taskhandle, 0, eSetValueWithOverwrite);
            return true;
        }
        return false;

    } else if (ep_dir == TUSB_DIR_OUT) {
        if (xferred_bytes >= 0u && xferred_bytes <= DAP_PACKET_SIZE) {
            xSemaphoreTake(edpt_lock, portMAX_DELAY);
            /* Leave OUT unarmed when the request ring is full; ph_fw_dap_thread
             * re-arms it after consuming a packet. */
            if (!buffer_full(&usb_request_buffer)) {
                usb_request_buffer.write_idx++;
                usbd_edpt_xfer(rhport, ep_addr, PH_FW_WR_SLOT_PTR(usb_request_buffer), DAP_PACKET_SIZE);
                usb_request_buffer.was_full = false;
            } else {
                usb_request_buffer.was_full = true;
            }
            xSemaphoreGive(edpt_lock);

            xTaskNotify(dap_taskhandle, 0, eSetValueWithOverwrite);
            return true;
        }
        return false;
    }
    return false;
}

static void ph_fw_dap_thread(void *ptr)
{
    uint32_t n;
    uint32_t cmd;
    uint16_t resp_len;

    do {
        /* USB callbacks wake this task when a request or IN slot is ready. */
        xTaskNotifyWait(0, 0xFFFFFFFFu, &cmd, 1);

        while (usb_request_buffer.read_idx != usb_request_buffer.write_idx) {
            /*
             * Atomic command support - buffer QueueCommands, but don't process them
             * until a non-QueueCommands packet is seen.
             */
            n = usb_request_buffer.read_idx;
            while (usb_request_buffer.data[n % DAP_PACKET_COUNT][0] == ID_DAP_QueueCommands) {
                ph_fw_probe_info("%lu %lu DAP queued cmd %s len %02x\n", usb_request_buffer.write_idx,
                           usb_request_buffer.read_idx,
                           dap_cmd_string[usb_request_buffer.data[n % DAP_PACKET_COUNT][0]],
                           usb_request_buffer.data[n % DAP_PACKET_COUNT][1]);
                usb_request_buffer.data[n % DAP_PACKET_COUNT][0] = ID_DAP_ExecuteCommands;
                n++;
                while (n == usb_request_buffer.write_idx) {
                    /* Need yield in a loop here, as IN callbacks will also wake the thread */
                    ph_fw_probe_info("DAP wait\n");
                    vTaskSuspend(dap_taskhandle);
                }
            }
            /* Read the next request from the ring. */
            ph_fw_probe_info("%lu %lu DAP cmd %s len %02x\n", usb_request_buffer.write_idx,
                       usb_request_buffer.read_idx,
                       dap_cmd_string[*PH_FW_RD_SLOT_PTR(usb_request_buffer)],
                       *(PH_FW_RD_SLOT_PTR(usb_request_buffer) + 1));

            /* A consumed slot lets the OUT endpoint receive again. */
            xSemaphoreTake(edpt_lock, portMAX_DELAY);
            if (usb_request_buffer.was_full) {
                usb_request_buffer.write_idx++;
                usbd_edpt_xfer(dap_rhport, out_ep_addr, PH_FW_WR_SLOT_PTR(usb_request_buffer),
                               DAP_PACKET_SIZE);
                usb_request_buffer.was_full = false;
            }
            xSemaphoreGive(edpt_lock);

            resp_len = DAP_ExecuteCommand(PH_FW_RD_SLOT_PTR(usb_request_buffer),
                                          PH_FW_WR_SLOT_PTR(usb_response_buffer)) &
                       0xffff;
            usb_request_buffer.read_idx++;
            ph_fw_probe_info("%lu %lu DAP resp %s len %u\n", usb_response_buffer.write_idx,
                       usb_response_buffer.read_idx,
                       dap_cmd_string[*PH_FW_WR_SLOT_PTR(usb_response_buffer)], resp_len);

            usb_response_buffer.data_len[PH_FW_WR_IDX(usb_response_buffer)] = resp_len;
            /* Publish the response and start IN if the endpoint was idle. */
            xSemaphoreTake(edpt_lock, portMAX_DELAY);

            if (buffer_empty(&usb_response_buffer)) {
                usb_response_buffer.write_idx++;

                usbd_edpt_xfer(dap_rhport, in_ep_addr, PH_FW_RD_SLOT_PTR(usb_response_buffer),
                               usb_response_buffer.data_len[PH_FW_RD_IDX(usb_response_buffer)]);
            } else {
                usb_response_buffer.write_idx++;

                /* The IN callback needs to check this flag to know when to queue up the
                 * next buffer. */
                usb_response_buffer.was_empty = false;
            }
            xSemaphoreGive(edpt_lock);
        }
    } while (1);
}

/* Add the custom driver to the tinyUSB stack */
usbd_class_driver_t const *usbd_app_driver_get_cb(uint8_t *driver_count)
{
    *driver_count = 1;
    return &dap_edpt_driver;
}
