/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/sync.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "pb_decode.h"
#include "pb_encode.h"

#include "ph_bulk.h"
#include "ph_console.h"
#include "ph_terminal.h"
#include "ph_fw_target.h"
#include "ph_fw_util.h"
#include "ph_fw_worker.h"

#define PH_TX_RING 1024u
#define PH_TRACE_RING 8192u

/* Short GetInfo identity; fits the 64-byte nanopb string field. */
#define PH_INFO_STRING "PicoHarness/6;ph-terminal;ph-t-bulk;la-sump;sump2;ut1;pc1;" PH_FW_DEBUG_CORE
_Static_assert(sizeof(PH_INFO_STRING) <= sizeof(((ph_get_info_t *)0)->info),
               "PH identity must fit the nanopb GetInfo field");

/* ------------------------------------------------------------------ */
/* core1 -> core0 CDC0 TX ring (SPSC). core1 appends, core0 flushes. */
/* ------------------------------------------------------------------ */
static uint8_t tx_ring[PH_TX_RING] __attribute__((aligned(4)));
static volatile uint32_t tx_head, tx_tail;
static volatile uint32_t drops;
/* Separate SPSC trace queue: records are uint16 length + complete PH envelope.
 * Only core0 writes CDC; it must finish a partially sent trace envelope before
 * resuming control bytes so the two streams never interleave mid-message. */
static uint8_t trace_ring[PH_TRACE_RING];
static volatile uint32_t trace_head, trace_tail;
static uint32_t trace_left, trace_omitted;

static bool ph_flush_trace_record(void)
{
    while (trace_left && tud_cdc_n_connected(0)) {
        uint32_t offset = trace_tail & (PH_TRACE_RING - 1);
        uint32_t count = PH_TRACE_RING - offset;
        if (count > trace_left) count = trace_left;
        uint32_t written = tud_cdc_n_write(0, trace_ring + offset, count);
        if (!written) break;
        __dmb();
        trace_tail += written;
        trace_left -= written;
    }
    return trace_left == 0;
}

static void ph_flush_tx(void)
{
    if (!ph_flush_trace_record())
        goto flush;
    while (tx_head != tx_tail && tud_cdc_n_connected(0)) {
        uint32_t offset = tx_tail & (PH_TX_RING - 1);
        uint32_t count = tx_head - tx_tail;
        if (offset + count > PH_TX_RING)
            count = PH_TX_RING - offset;
        if (count == 0)
            break;
        uint32_t written = tud_cdc_n_write(0, tx_ring + offset, count);
        if (!written)
            break;
        __dmb();
        tx_tail += written;
    }
    /* Begin trace records only after queued control bytes were drained. */
    while (tx_head == tx_tail && trace_head != trace_tail && tud_cdc_n_connected(0)) {
        __dmb();
        trace_left = trace_ring[trace_tail & (PH_TRACE_RING - 1)] |
            ((uint32_t)trace_ring[(trace_tail + 1) & (PH_TRACE_RING - 1)] << 8);
        __dmb();
        trace_tail += 2;
        if (!ph_flush_trace_record()) break;
    }
flush:
    if (tud_cdc_n_write_available(0))
        tud_cdc_n_write_flush(0);
}

void ph_terminal_send(const uint8_t *bytes, size_t len)
{
    uint32_t deadline = time_us_32() + 2000; /* core0 drains every loop */
    while (PH_TX_RING - (tx_head - tx_tail) < len && tud_cdc_n_connected(0)) {
        if (time_us_32() >= deadline) {
            if (PH_TX_RING - (tx_head - tx_tail) < len)
                break; /* stalled: drop rather than block USB servicing */
        }
    }
    if (PH_TX_RING - (tx_head - tx_tail) < len) {
        drops++;
        return;
    }
    uint32_t offset = tx_head & (PH_TX_RING - 1);
    size_t first = PH_TX_RING - offset;
    if (first > len)
        first = len;
    memcpy(tx_ring + offset, bytes, first);
    if (first < len)
        memcpy(tx_ring, bytes + first, len - first);
    __dmb();
    tx_head += (uint32_t)len;
}

void ph_terminal_init(void)
{
    ph_terminal_reset();
}

/* ------------------------------------------------------------------ */
/* core0: RX framing for the length-delimited CDC0 stream and job mbox */
/* ------------------------------------------------------------------ */
static uint8_t rx_buf[PH_MSG_MAX];
static size_t rx_len;

#define PH_JOB_IDLE 0
#define PH_JOB_READY 1
static volatile uint32_t job_state;
static uint8_t job_buf[PH_MSG_MAX];
static uint16_t job_len;

/* Return true when rx_buf holds one complete '<varint len><body>' frame;
 * sets *start to the body offset and *len to the body length. */
static bool frame_ready(size_t *start, size_t *len)
{
    if (rx_len == 0)
        return false;

    uint32_t varint = 0;
    size_t v = 0;
    for (; v < PH_VARINT_MAX && v < rx_len; ++v) {
        varint |= (uint32_t)(rx_buf[v] & 0x7f) << (7 * v);
        if (!(rx_buf[v] & 0x80)) {
            v++;
            break;
        }
    }
    if (v >= PH_VARINT_MAX || v > rx_len)
        return false; /* resync: caller drops bytes until a clean prefix */
    if (varint == 0 || varint > PH_MSG_MAX)
        return false;

    size_t total = v + (size_t)varint;
    if (rx_len < total)
        return false; /* incomplete frame */

    *start = v;
    *len = (size_t)varint;
    return true;
}

static void consume(size_t total)
{
    if (total >= rx_len) {
        rx_len = 0;
        return;
    }
    memmove(rx_buf, rx_buf + total, rx_len - total);
    rx_len -= total;
}

uint32_t ph_terminal_pump(void)
{
    uint32_t consumed = 0;

    if (tud_cdc_n_available(0)) {
        uint32_t room = (uint32_t)(PH_MSG_MAX - rx_len);
        uint32_t n = tud_cdc_n_read(0, rx_buf + rx_len, room);
        consumed += n;
        rx_len += n;
    }

    size_t start, len;
    while (frame_ready(&start, &len)) {
        if (job_state == PH_JOB_IDLE && tud_cdc_n_connected(0)) {
            memcpy(job_buf, rx_buf + start, len);
            job_len = (uint16_t)len;
            __dmb();
            job_state = PH_JOB_READY;
            __dmb();
        }
        consume(start + len);
    }

    ph_flush_tx();
    ph_bulk_flush_tx();
    return consumed;
}

/* ------------------------------------------------------------------ */
/* encoding helpers                                                  */
/* ------------------------------------------------------------------ */

static void emit(const ph_msg_t *msg)
{
    uint8_t out[PH_MSG_MAX];
    pb_ostream_t stream = pb_ostream_from_buffer(out, sizeof(out));
    if (pb_encode_delimited(&stream, &picoharness_v1_PhMessage_msg, msg))
        ph_terminal_send(out, stream.bytes_written);
}

void ph_send_status(uint32_t seq, ph_status_t status)
{
    ph_msg_t msg = picoharness_v1_PhMessage_init_default;
    msg.seq = seq;
    msg.status = status;
    emit(&msg);
}

void ph_send_body(uint32_t seq, ph_status_t status, pb_size_t body_tag,
                  const void *body, size_t body_size)
{
    ph_msg_t msg = picoharness_v1_PhMessage_init_default;
    msg.seq = seq;
    msg.status = status;
    msg.which_body = body_tag;
    if (body_size > sizeof(msg.body))
        body_size = sizeof(msg.body);
    memcpy(&msg.body, body, body_size);
    emit(&msg);
}

/* Trace uses its own bounded queue and never waits for USB space. Control
 * traffic remains prioritized; report omissions in the next delivered record. */
void ph_uart_trace_emit(bool tx, uint32_t baud, const uint8_t *bytes, size_t len)
{
    while (len) {
        size_t n = len > 128 ? 128 : len;
        ph_msg_t msg = picoharness_v1_PhMessage_init_default;
        msg.which_body = picoharness_v1_PhMessage_uart_trace_tag;
        picoharness_v1_UartTrace *trace = &msg.body.uart_trace;
        trace->tx = tx;
        trace->baud = baud;
        trace->timestamp_us = time_us_64();
        trace->dropped = trace_omitted;
        trace->data.size = n;
        memcpy(trace->data.bytes, bytes, n);
        uint8_t out[PH_MSG_MAX];
        pb_ostream_t stream = pb_ostream_from_buffer(out, sizeof(out));
        if (!pb_encode_delimited(&stream, &picoharness_v1_PhMessage_msg, &msg) ||
            PH_TRACE_RING - (trace_head - trace_tail) < stream.bytes_written + 2) {
            trace_omitted += n;
        } else {
            uint32_t head = trace_head;
            trace_ring[head++ & (PH_TRACE_RING - 1)] = (uint8_t)stream.bytes_written;
            trace_ring[head++ & (PH_TRACE_RING - 1)] = (uint8_t)(stream.bytes_written >> 8);
            for (size_t i = 0; i < stream.bytes_written; ++i)
                trace_ring[head++ & (PH_TRACE_RING - 1)] = out[i];
            __dmb();
            trace_head = head;
        }
        bytes += n;
        len -= n;
    }
}

void ph_console_data_emit(const uint8_t *bytes, size_t len)
{
    ph_console_data_t data = picoharness_v1_ConsoleData_init_default;
    if (len > sizeof(data.data.bytes))
        len = sizeof(data.data.bytes);
    data.data.size = len;
    memcpy(data.data.bytes, bytes, len);
    ph_send_body(0, PH_STATUS_OK, picoharness_v1_PhMessage_console_data_tag,
                 &data, sizeof(data));
}

/* ------------------------------------------------------------------ */
/* executors                                                         */
/* ------------------------------------------------------------------ */

static void exec_get_info(uint32_t seq)
{
    ph_get_info_t info = picoharness_v1_GetInfo_init_default;
    memcpy(info.info, PH_INFO_STRING, sizeof(PH_INFO_STRING));
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_get_info_tag,
                 &info, sizeof(info));
}

static void exec_get_status(uint32_t seq)
{
    ph_get_status_t st = picoharness_v1_GetStatus_init_default;
    st.power_enabled = gpio_get(PH_FW_WORK_ENABLE_PIN) != 0;
    uint16_t mv = ph_fw_target_work_mv();
    st.power_good = mv >= 2800;
    st.ina_alert = !gpio_get(PH_FW_INA_ALERT_PIN);
    st.work_fault = !gpio_get(PH_FW_WORK_FAULT_PIN);
    st.vbus_mv = mv;
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_get_status_tag,
                 &st, sizeof(st));
}

static void exec_power_set(uint32_t seq, const ph_power_set_t *ps)
{
    ph_fw_target_power_set(ps->enabled);
    ph_send_status(seq, PH_STATUS_OK);
}

static void exec_reset_pulse(uint32_t seq, const ph_reset_pulse_t *rp)
{
    if (rp->duration_ms == 0 || rp->duration_ms > 1000) {
        ph_send_status(seq, PH_STATUS_ERR_ARG);
        return;
    }
    gpio_put(PH_FW_WORK_RUN_PIN, true);
    gpio_set_dir(PH_FW_WORK_RUN_PIN, GPIO_OUT);
    busy_wait_ms(rp->duration_ms);
    gpio_put(PH_FW_WORK_RUN_PIN, false);
    ph_send_status(seq, PH_STATUS_OK);
}

static void exec_bootsel_set(uint32_t seq, const ph_bootsel_set_t *bs)
{
    gpio_put(PH_FW_WORK_BOOT_PIN, bs->asserted);
    gpio_set_dir(PH_FW_WORK_BOOT_PIN, GPIO_OUT);
    ph_send_status(seq, PH_STATUS_OK);
}

static void exec_enter_bootsel(uint32_t seq)
{
    if (!ph_fw_target_io_ready()) {
        ph_send_status(seq, PH_STATUS_ERR_NOTREADY);
        return;
    }
    gpio_put(PH_FW_WORK_RUN_PIN, true);
    gpio_set_dir(PH_FW_WORK_RUN_PIN, GPIO_OUT);
    gpio_put(PH_FW_WORK_BOOT_PIN, true);
    gpio_set_dir(PH_FW_WORK_BOOT_PIN, GPIO_OUT);
    busy_wait_ms(10);
    gpio_put(PH_FW_WORK_RUN_PIN, false);
    busy_wait_ms(10);
    gpio_put(PH_FW_WORK_BOOT_PIN, false);
    ph_send_status(seq, PH_STATUS_OK);
}

static void exec_measure(uint32_t seq)
{
    int32_t values[4];
    if (!ph_fw_target_power_measure(values)) {
        ph_send_status(seq, PH_STATUS_ERR_NOTREADY);
        return;
    }
    ph_measure_t m = picoharness_v1_Measure_init_default;
    m.shunt_uv = values[0];
    m.vbus_uv = values[1];
    m.current_ua = values[2];
    m.power_uw = values[3];
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_measure_tag,
                 &m, sizeof(m));
}

static void exec_gpio_read(uint32_t seq)
{
    ph_gpio_read_t g = picoharness_v1_GpioRead_init_default;
    g.mask = ph_fw_normalize_gpio(gpio_get_all());
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_gpio_read_tag,
                 &g, sizeof(g));
}

static void exec_gpio_diag(uint32_t seq, const ph_gpio_diag_t *gd)
{
    if (gd->pin >= 30) {
        ph_send_status(seq, PH_STATUS_ERR_ARG);
        return;
    }
    uint pin = gd->pin;
    ph_gpio_diag_t out = picoharness_v1_GpioDiag_init_default;
    out.pin = pin;
    out.level = gpio_get(pin);
    out.output = gpio_get_dir(pin) == GPIO_OUT;
    out.function = (uint32_t)gpio_get_function(pin);
    out.pull_up = gpio_is_pulled_up(pin);
    out.pull_down = gpio_is_pulled_down(pin);
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_gpio_diag_tag,
                 &out, sizeof(out));
}

static void exec_console_tx(uint32_t seq, const ph_console_tx_t *tx)
{
    if (!ph_fw_target_io_ready()) {
        ph_send_status(seq, PH_STATUS_ERR_NOTREADY);
        return;
    }
    ph_console_write(tx->data.bytes, tx->data.size);
    ph_send_status(seq, PH_STATUS_OK);
}

static void exec_term_config(uint32_t seq, const ph_term_config_t *tc)
{
    uint32_t actual;
    if (!ph_console_baud(tc->baud, &actual)) {
        ph_send_status(seq, PH_STATUS_ERR_ARG);
        return;
    }
    ph_term_config_t out = picoharness_v1_TermConfig_init_default;
    out.baud = tc->baud;
    out.data_bits = tc->data_bits;
    out.stop_bits = tc->stop_bits;
    out.parity = tc->parity;
    out.actual_baud = actual;
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_term_config_tag,
                 &out, sizeof(out));
}

static void exec_term_status(uint32_t seq)
{
    uint16_t rx_available = 0, tx_space = 0;
    uint32_t overruns = 0;
    /* FIFO counters are best-effort; the mode/baud report always answers so a
     * host can tell plain-UART from pico_harness without probing Work. */
    ph_fw_target_serial_status(0, &rx_available, &tx_space, &overruns);
    ph_term_status_t st = picoharness_v1_TermStatus_init_default;
    st.rx_available = rx_available;
    st.tx_space = tx_space;
    st.overruns = overruns;
    st.mode = ph_console_mode() == PH_CONSOLE_HARNESS
                  ? picoharness_v1_ConsoleMode_CONSOLE_HARNESS
                  : picoharness_v1_ConsoleMode_CONSOLE_PLAIN;
    st.actual_baud = ph_console_baud_get();
    ph_send_body(seq, PH_STATUS_OK, picoharness_v1_PhMessage_term_status_tag,
                 &st, sizeof(st));
}

static void exec_bulk_tx_begin(uint32_t seq, const ph_bulk_tx_begin_t *b)
{
    if (ph_bulk_busy()) {
        ph_send_status(seq, PH_STATUS_ERR_BUSY);
        return;
    }
    if (!ph_bulk_rx_begin(b->route, b->length, b->crc32c)) {
        ph_send_status(seq, PH_STATUS_ERR_ARG);
        return;
    }
    ph_send_status(seq, PH_STATUS_OK);
}

void ph_msg_execute(const uint8_t *payload, size_t len)
{
    ph_msg_t req = picoharness_v1_PhMessage_init_default;
    pb_istream_t is = pb_istream_from_buffer(payload, len);
    if (!pb_decode(&is, &picoharness_v1_PhMessage_msg, &req)) {
        ph_send_status(0, PH_STATUS_ERR_FRAME);
        return;
    }

    switch (req.which_body) {
    case picoharness_v1_PhMessage_console_tx_tag:
        exec_console_tx(req.seq, &req.body.console_tx);
        break;
    case picoharness_v1_PhMessage_term_config_tag:
        exec_term_config(req.seq, &req.body.term_config);
        break;
    case picoharness_v1_PhMessage_term_status_tag:
        exec_term_status(req.seq);
        break;
    case picoharness_v1_PhMessage_get_info_tag:
        exec_get_info(req.seq);
        break;
    case picoharness_v1_PhMessage_get_status_tag:
        exec_get_status(req.seq);
        break;
    case picoharness_v1_PhMessage_power_set_tag:
        exec_power_set(req.seq, &req.body.power_set);
        break;
    case picoharness_v1_PhMessage_reset_pulse_tag:
        exec_reset_pulse(req.seq, &req.body.reset_pulse);
        break;
    case picoharness_v1_PhMessage_bootsel_set_tag:
        exec_bootsel_set(req.seq, &req.body.bootsel_set);
        break;
    case picoharness_v1_PhMessage_enter_bootsel_tag:
        exec_enter_bootsel(req.seq);
        break;
    case picoharness_v1_PhMessage_measure_tag:
        exec_measure(req.seq);
        break;
    case picoharness_v1_PhMessage_gpio_read_tag:
        exec_gpio_read(req.seq);
        break;
    case picoharness_v1_PhMessage_gpio_diag_tag:
        exec_gpio_diag(req.seq, &req.body.gpio_diag);
        break;
    case picoharness_v1_PhMessage_debug_bootsel_tag:
        ph_fw_request_self_bootsel();
        ph_send_status(req.seq, PH_STATUS_OK);
        break;
    case picoharness_v1_PhMessage_bulk_tx_begin_tag:
        exec_bulk_tx_begin(req.seq, &req.body.bulk_tx_begin);
        break;
    default:
        ph_send_status(req.seq, PH_STATUS_ERR_CMD);
        break;
    }
}

/* ------------------------------------------------------------------ */
/* core1: job polling                                                */
/* ------------------------------------------------------------------ */
bool ph_terminal_worker(void)
{
    if (job_state != PH_JOB_READY)
        return false;
    __dmb();
    uint8_t local[PH_MSG_MAX];
    uint16_t len = job_len;
    memcpy(local, job_buf, len);
    __dmb();
    job_state = PH_JOB_IDLE;
    __dmb();
    ph_msg_execute(local, len);
    return true;
}

void ph_terminal_reset(void)
{
    tx_head = tx_tail = 0;
    trace_head = trace_tail = trace_left = trace_omitted = 0;
    rx_len = 0;
    job_state = PH_JOB_IDLE;
}