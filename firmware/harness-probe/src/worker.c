/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/clocks.h"
#include "hardware/spi.h"
#include "hardware/sync.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "logic_analyzer.h"
#include "target.h"
#include "util.h"
#include "worker.h"

#define PDBG_MAGIC 0x47424450u /* "PDBG", little endian */
#define PDBG_VERSION 1
#define PDBG_MAX_PAYLOAD 4096
#define CAPTURE_SAMPLES 32768

enum pdbg_command {
    PDBG_GET_INFO = 1,
    PDBG_GET_STATUS = 2,
    PDBG_POWER_SET = 3,
    PDBG_RESET_PULSE = 4,
    PDBG_BOOTSEL_SET = 5,
    PDBG_ENTER_BOOTSEL = 6,
    PDBG_MEASURE = 7,
    PDBG_GPIO_READ = 8,
    PDBG_CAPTURE = 9,
    PDBG_CAPTURE_READ = 10,
    PDBG_GPIO_DIAG = 11,
    PDBG_GPIO_PULL = 12,
    PDBG_UART_CONFIG = 13,
    PDBG_UART_WRITE = 14,
    PDBG_UART_READ = 15,
    PDBG_UART_STATUS = 16,
    PDBG_SPI_CONFIG = 17,
    PDBG_SPI_TRANSFER = 18,
    PDBG_CAPTURE_STATUS = 19,
    PDBG_DEBUG_BOOTSEL = 20,
    PDBG_CAPTURE_START = 21,
    PDBG_CAPTURE_POLL = 22,
    PDBG_CAPTURE_CANCEL = 23,
};

enum pdbg_status { PDBG_OK = 0, PDBG_BAD_FRAME = 1, PDBG_BAD_COMMAND = 2, PDBG_BAD_ARGUMENT = 3 };

enum { JOB_IDLE, JOB_READY, JOB_DONE };

typedef struct __attribute__((packed)) {
    uint32_t magic;
    uint8_t version;
    uint8_t command;
    uint16_t flags;
    uint32_t sequence;
    uint16_t length;
    uint16_t header_crc;
} pdbg_header_t;

/* Core 0 owns USB buffers; core 1 owns job_input while a job is running. */
static uint8_t rx[PDBG_MAX_PAYLOAD + sizeof(pdbg_header_t) + 4];
static uint8_t response_payload[PDBG_MAX_PAYLOAD];
static uint8_t response_packet[sizeof(pdbg_header_t) + PDBG_MAX_PAYLOAD + 4];
static size_t rx_count;
static uint32_t capture_count;
static uint32_t capture_id;
static uint32_t capture_elapsed_us;
static la_result_t capture_result;

/* Single producer / single consumer mailbox. Each state publication is
 * preceded by DMB; neither side touches payloads owned by the other. */
static volatile uint32_t job_state, connection_epoch;
static uint32_t job_epoch;
static pdbg_header_t job_header;
static uint8_t job_input[PDBG_MAX_PAYLOAD];
static uint16_t job_output_length;
static size_t response_size, response_sent;
static volatile bool debug_boot_requested;
static uint32_t debug_boot_deadline; /* owned by core 0 */

static void control_assert(uint pin, bool asserted)
{
    gpio_put(pin, asserted);
    gpio_set_dir(pin, GPIO_OUT);
}

static bool spi_job_yield(void)
{
    target_serial_poll();

    /* Abort a transfer whose USB connection was reset behind the job. */
    return connection_epoch != job_epoch;
}

void worker_init(void)
{
    /* target_io_init leaves cross-domain pins high-Z. RP2350 does not
     * guarantee digital input buffers on unused pads, so enable them for
     * every capture pin without changing its function or output direction. */
    for (uint pin = 0; pin < 30; ++pin) {
        if (PDBG_GPIO_RAW_MASK & (1u << pin))
            gpio_set_input_enabled(pin, true);
    }

    la_init();
}

static uint16_t execute(uint8_t command, const uint8_t *input, uint16_t length, uint8_t *output)
{
    output[0] = PDBG_OK;
    switch (command) {
    case PDBG_GET_INFO: {
        static const char info[] = PDBG_INFO_STRING;
        memcpy(output + 1, info, sizeof(info));
        return 1 + sizeof(info);
    }
    case PDBG_GET_STATUS: {
        output[1] = gpio_get(PDBG_WORK_ENABLE_PIN);
        uint16_t millivolts = target_work_mv();
        output[2] = millivolts >= 2800;
        output[3] = !gpio_get(PDBG_INA_ALERT_PIN);
        output[4] = !gpio_get(PDBG_WORK_FAULT_PIN);
        memcpy(output + 5, &millivolts, sizeof(millivolts));
        return 7;
    }
    case PDBG_POWER_SET:
        if (length != 1 || input[0] > 1)
            break;
        target_power_set(input[0] != 0);
        return 1;
    case PDBG_RESET_PULSE: {
        if (length != 0 && length != 2)
            break;
        uint16_t ms = length == 2 ? (uint16_t)(input[0] | (input[1] << 8)) : 10;
        if (ms == 0 || ms > 1000)
            break;
        control_assert(PDBG_WORK_RUN_PIN, true);
        busy_wait_ms(ms);
        control_assert(PDBG_WORK_RUN_PIN, false);
        return 1;
    }
    case PDBG_BOOTSEL_SET:
        if (length != 1)
            break;
        control_assert(PDBG_WORK_BOOT_PIN, input[0] != 0);
        return 1;
    case PDBG_ENTER_BOOTSEL:
        if (length != 0)
            break;
        if (!target_io_ready()) {
            output[0] = PDBG_BAD_ARGUMENT;
            return 1;
        }
        control_assert(PDBG_WORK_RUN_PIN, true);
        control_assert(PDBG_WORK_BOOT_PIN, true);
        busy_wait_ms(10);
        control_assert(PDBG_WORK_RUN_PIN, false);
        busy_wait_ms(10);
        control_assert(PDBG_WORK_BOOT_PIN, false);
        return 1;
    case PDBG_MEASURE: {
        int32_t values[4];
        if (!target_power_measure(values)) {
            output[0] = PDBG_BAD_FRAME;
            return 1;
        }
        memcpy(output + 1, values, sizeof(values));
        return 1 + sizeof(values);
    }
    case PDBG_GPIO_READ: {
        uint32_t sample = normalize_gpio(gpio_get_all());
        memcpy(output + 1, &sample, sizeof(sample));
        return 5;
    }
    case PDBG_CAPTURE:
    case PDBG_CAPTURE_START: {
        uint32_t count, frequency;

        if (length != 8 && length != 14)
            break;
        memcpy(&count, input, 4);
        memcpy(&frequency, input + 4, 4);

        if (count == 0 || count > CAPTURE_SAMPLES || frequency < 2500 ||
            frequency > clock_get_hz(clk_sys))
            break;

        uint8_t trigger = length == 14 ? input[8] : 0;
        uint8_t raw_pin = 0;
        uint16_t timeout_ms = length == 14 ? (uint16_t)(input[10] | input[11] << 8) : 0;
        bool bypass = length == 14 && (input[12] & 1);
        if (length == 14 && (input[12] > 1 || input[13]))
            break;
        if (trigger) {
            bool found = false;
#define FIND_PIN(debug_pin, work_pin) \
    if (input[9] == work_pin) {       \
        raw_pin = debug_pin;          \
        found = true;                 \
    }
            PDBG_GPIO_MAP(FIND_PIN)
#undef FIND_PIN
            if (!found)
                break;
        }

        bool valid = command == PDBG_CAPTURE_START ?
            la_start(count, frequency, trigger, raw_pin, timeout_ms, bypass, &connection_epoch, job_epoch, &capture_result) :
            la_capture(count, frequency, trigger, raw_pin, timeout_ms, bypass, &connection_epoch, job_epoch, &capture_result);
        if (!valid)
            break;

        capture_count = capture_result.count;
        capture_elapsed_us = capture_result.elapsed_us;
        capture_id++;
        memcpy(output + 1, &capture_id, 4);
        if (command == PDBG_CAPTURE_START)
            return 5;
        memcpy(output + 5, &capture_count, 4);
        memcpy(output + 9, &capture_elapsed_us, 4);
        return 13;
    }
    case PDBG_CAPTURE_READ: {
        uint32_t id, offset;
        uint16_t count;
        if (length != 10 || la_busy())
            break;
        memcpy(&id, input, 4);
        memcpy(&offset, input + 4, 4);
        memcpy(&count, input + 8, 2);
        if (id != capture_id || count > (PDBG_MAX_PAYLOAD - 1) / 4 || offset > capture_count ||
            count > capture_count - offset)
            break;
        for (uint32_t i = 0; i < count; ++i) {
            uint32_t sample = la_sample(offset + i);
            memcpy(output + 1 + i * 4, &sample, 4);
        }
        return (uint16_t)(1 + count * 4);
    }
    case PDBG_CAPTURE_POLL:
        if (length)
            break;
        memcpy(output + 1, &capture_id, 4);
        memcpy(output + 5, &capture_result, sizeof(capture_result));
        return 5 + sizeof(capture_result);
    case PDBG_CAPTURE_CANCEL:
        if (length)
            break;
        la_cancel();
        return 1;
    case PDBG_CAPTURE_STATUS:
        if (length)
            break;
        memcpy(output + 1, &capture_result, sizeof(capture_result));
        return 1 + sizeof(capture_result);
    case PDBG_DEBUG_BOOTSEL:
        if (length != 4 || memcmp(input, "BOOT", 4))
            break;
        debug_boot_requested = true;
        return 1;
    case PDBG_GPIO_DIAG: {
        if (length != 1 || input[0] >= 30)
            break;
        uint pin = input[0];
        output[1] = gpio_get(pin);
        output[2] = gpio_get_dir(pin) == GPIO_OUT;
        output[3] = (uint8_t)gpio_get_function(pin);
        output[4] = gpio_is_pulled_up(pin);
        output[5] = gpio_is_pulled_down(pin);
        return 6;
    }
    case PDBG_GPIO_PULL:
        output[0] = PDBG_BAD_COMMAND;
        return 1;
    case PDBG_UART_CONFIG: {
        uint32_t baud, actual_baud;
        if (length != 8)
            break;
        memcpy(&baud, input + 1, sizeof(baud));
        if (!target_serial_config(input[0], baud, input[5], input[6], input[7], &actual_baud))
            break;
        memcpy(output + 1, &actual_baud, sizeof(actual_baud));
        return 5;
    }
    case PDBG_UART_WRITE: {
        if (!target_io_ready())
            break;
        if (length < 1 || input[0] >= PROBE_UART_COUNT)
            break;
        uint16_t accepted = (uint16_t)target_serial_write(input[0], input + 1, length - 1);
        memcpy(output + 1, &accepted, sizeof(accepted));
        return 3;
    }
    case PDBG_UART_READ: {
        uint16_t requested;
        if (length != 3 || input[0] >= PROBE_UART_COUNT)
            break;
        memcpy(&requested, input + 1, sizeof(requested));
        if (requested > PDBG_MAX_PAYLOAD - 1)
            break;
        size_t count = target_serial_read(input[0], output + 1, requested);
        return (uint16_t)(1 + count);
    }
    case PDBG_UART_STATUS: {
        uint16_t rx_available, tx_space;
        uint32_t rx_overruns;
        if (length != 1 || !target_serial_status(input[0], &rx_available, &tx_space, &rx_overruns))
            break;
        memcpy(output + 1, &rx_available, sizeof(rx_available));
        memcpy(output + 3, &tx_space, sizeof(tx_space));
        memcpy(output + 5, &rx_overruns, sizeof(rx_overruns));
        return 9;
    }
    case PDBG_SPI_CONFIG: {
        uint32_t baud, actual_baud;
        if (length != 5 || input[4] > 3)
            break;
        memcpy(&baud, input, sizeof(baud));
        if (baud < 10000 || baud > clock_get_hz(clk_sys) / 2)
            break;
        actual_baud = spi_set_baudrate(PDBG_SPI_INTERFACE, baud);
        spi_set_format(PDBG_SPI_INTERFACE, 8, (input[4] & 2) ? SPI_CPOL_1 : SPI_CPOL_0,
                       (input[4] & 1) ? SPI_CPHA_1 : SPI_CPHA_0, SPI_MSB_FIRST);
        memcpy(output + 1, &actual_baud, sizeof(actual_baud));
        return 5;
    }
    case PDBG_SPI_TRANSFER:
        if (!target_io_ready())
            break;
        if (length == 0 || length > PDBG_MAX_PAYLOAD - 1)
            break;
        if (!target_spi_transfer(input, output + 1, length, spi_job_yield)) {
            output[0] = PDBG_BAD_ARGUMENT;
            return 1;
        }
        return (uint16_t)(1 + length);
    default:
        output[0] = PDBG_BAD_COMMAND;
        return 1;
    }
    output[0] = PDBG_BAD_ARGUMENT;
    return 1;
}

static void send_response(const pdbg_header_t *request, const uint8_t *payload, uint16_t length)
{
    pdbg_header_t header = *request;

    header.command |= 0x80;
    header.flags = 0;
    header.length = length;
    header.header_crc = 0;
    header.header_crc = crc16_ccitt((uint8_t *)&header, sizeof(header));

    memcpy(response_packet, &header, sizeof(header));
    memcpy(response_packet + sizeof(header), payload, length);
    uint32_t checksum = crc32c(payload, length);
    memcpy(response_packet + sizeof(header) + length, &checksum, 4);

    response_size = sizeof(header) + length + 4;
    response_sent = 0;
}

static bool flush_response(void)
{
    if (response_sent >= response_size)
        return false;

    response_sent +=
        tud_vendor_write(response_packet + response_sent, response_size - response_sent);
    tud_vendor_flush();
    return true;
}

static void parse_received_frames(void)
{
    while (rx_count >= sizeof(pdbg_header_t)) {
        pdbg_header_t header;
        memcpy(&header, rx, sizeof(header));

        /* Resynchronize one byte at a time after an invalid header. */
        if (header.magic != PDBG_MAGIC || header.version != PDBG_VERSION ||
            header.length > PDBG_MAX_PAYLOAD) {
            memmove(rx, rx + 1, --rx_count);
            continue;
        }

        size_t total = sizeof(header) + header.length + 4;
        if (rx_count < total)
            return;

        uint16_t saved_crc = header.header_crc;
        header.header_crc = 0;
        uint32_t payload_crc;
        memcpy(&payload_crc, rx + sizeof(header) + header.length, 4);
        bool valid = saved_crc == crc16_ccitt((uint8_t *)&header, sizeof(header)) &&
                     payload_crc == crc32c(rx + sizeof(header), header.length);

        if (valid) {
            job_header = header;
            memcpy(job_input, rx + sizeof(header), header.length);
            job_epoch = connection_epoch;
            __dmb();
            job_state = JOB_READY;
        }

        rx_count -= total;
        memmove(rx, rx + total, rx_count);
        if (valid)
            return;
    }
}

void worker_step(void)
{
    /* Core 0 owns board monitoring, power policy, and the LED.  Core 1 only
     * publishes the job state and accesses the protected I/O engines. */
    target_control_poll();

    switch (job_state) {
    case JOB_DONE:
        __dmb();
        if (job_epoch == connection_epoch && tud_vendor_mounted())
            send_response(&job_header, response_payload, job_output_length);
        __dmb();
        job_state = JOB_IDLE;
        /* fall through */
    case JOB_IDLE:
        if (flush_response())
            return;

        if (debug_boot_requested) {
            /* A response has been queued; allow USB to transmit it before reset. */
            if (!debug_boot_deadline)
                debug_boot_deadline = time_us_32() + 100000;
            if ((int32_t)(time_us_32() - debug_boot_deadline) >= 0)
                reset_usb_boot(0, 0);
            return;
        }

        if (!tud_vendor_mounted())
            return;

        rx_count += tud_vendor_read(rx + rx_count, sizeof(rx) - rx_count);
        parse_received_frames();
        return;
    case JOB_READY:
        flush_response();
        return;
    default:
        return;
    }
}

void worker_reset(void)
{
    rx_count = 0;
    ++connection_epoch;
    response_size = response_sent = 0;
    /* An update request belongs to the connection that asked for it. */
    debug_boot_requested = false;
    debug_boot_deadline = 0;
    __dmb();
}

void worker_thread(void)
{
    /* Core 1 has no RTOS tick, USB ISR, DMA ISR or FIFO doorbell ISR. */
    save_and_disable_interrupts();

    for (;;) {
        target_serial_poll();
        if (la_poll()) {
            capture_count = capture_result.count;
            capture_elapsed_us = capture_result.elapsed_us;
        }
        if (job_state == JOB_READY) {
            __dmb();
            if (job_epoch == connection_epoch)
                job_output_length =
                    execute(job_header.command, job_input, job_header.length, response_payload);
            __dmb();
            job_state = JOB_DONE;
        }
        tight_loop_contents();
    }
}
