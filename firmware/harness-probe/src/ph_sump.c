/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/clocks.h"
#include "hardware/sync.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "ph_fw_la.h"
#include "ph_sump.h"
#include "ph_fw_target.h"
#include "ph_fw_util.h"

#define PH_FW_LA_CDC 1u
#define PH_FW_LA_CHANNELS 16u
#define PH_FW_TRIGGER_TIMEOUT_MS 5000u

/* SUMP channel -> work-pin bit position (the same PH_FW_GPIO_MAP order the
 * capture engine uses for `ph_fw_la_sample`'s normalized words). */
#define PH_SUMP_WPIN(debug_pin, work_pin) work_pin,
static const uint8_t sump_wpin[PH_FW_LA_CHANNELS] = { PH_FW_GPIO_MAP(PH_SUMP_WPIN) };
/* Trigger PIO observes Debug GPIO; output words are normalized Work GPIO. */
#define PH_SUMP_DPIN(debug_pin, work_pin) debug_pin,
static const uint8_t sump_dpin[PH_FW_LA_CHANNELS] = { PH_FW_GPIO_MAP(PH_SUMP_DPIN) };

/* SUMP registers. */
static uint32_t reg_divider, reg_count, reg_flags, reg_channels;
static uint32_t reg_mask, reg_value; /* stage-0 trigger mask/value */

/* core0 <-> core1 mailbox. */
static volatile uint8_t sump_req, sump_cancel, sump_done;
static volatile uint32_t sump_req_count, sump_req_hz, sump_req_raw_mask;
static volatile uint8_t sump_req_trigger, sump_req_pin;
static volatile uint32_t sump_actual, sump_flags, sump_sample_hz;
static volatile uint32_t sump_epoch /* volatile for ph_fw_la_start */ ;
static uint32_t sump_expected_bytes, sump_crc;
static bool sump_packed;
static uint32_t sump_expected; /* core0: samples to stream */
static uint8_t sump_streaming;
static uint32_t sump_stream_index;
static ph_fw_la_result_t sump_lares; /* core1 capture result slot */

/* SUMP command parser (max opcode+4 payload bytes). */
static uint8_t cmd_buf[5];
static uint8_t cmd_pos;
static uint8_t cmd_need;

static bool is_long(uint8_t op)
{
    return op == 0x80 || op == 0x81 || op == 0x82 || op == 0x83 || (op >= 0xc0 && op <= 0xcf);
}

static uint32_t le32(const uint8_t *b)
{
    return (uint32_t)b[0] | ((uint32_t)b[1] << 8) | ((uint32_t)b[2] << 16) |
           ((uint32_t)b[3] << 24);
}

/* Only core 0 touches CDC. Never wait here for completion callbacks: they
 * run in this same USB task. Keep an ID reply until the FIFO has room. */
static uint32_t id_pending;
static uint32_t status_pending;

static void sump_stream(void)
{
    if (sump_packed) {
        uint8_t bytes[64];
        uint32_t count = MIN(sump_expected_bytes - sump_stream_index,
                             MIN(sizeof(bytes), tud_cdc_n_write_available(PH_FW_LA_CDC)));
        for (uint32_t i = 0; i < count; ++i)
            bytes[i] = ph_fw_la_data_byte(sump_stream_index + i);
        uint32_t written = tud_cdc_n_write(PH_FW_LA_CDC, bytes, count);
        sump_crc = ph_fw_crc32c_update(sump_crc, bytes, written);
        sump_stream_index += written;
        if (sump_stream_index == sump_expected_bytes)
            tud_cdc_n_write_flush(PH_FW_LA_CDC);
        return;
    }
    uint8_t bytes[64];
    uint32_t count = MIN(sump_expected - sump_stream_index,
                         MIN(sizeof(bytes), tud_cdc_n_write_available(PH_FW_LA_CDC)) / 4);
    if (!count)
        return;
    for (uint32_t i = 0; i < count; ++i) {
        uint32_t idx = sump_stream_index + i;
        uint32_t s = idx < sump_actual ? ph_fw_la_sample(idx) : 0;
        uint32_t w = 0;
        for (uint32_t channel = 0; channel < PH_FW_LA_CHANNELS; ++channel)
            w |= ((s >> sump_wpin[channel]) & 1u) << channel;
        bytes[i * 4] = (uint8_t)w;
        bytes[i * 4 + 1] = (uint8_t)(w >> 8);
        bytes[i * 4 + 2] = (uint8_t)(w >> 16);
        bytes[i * 4 + 3] = (uint8_t)(w >> 24);
    }
    /* Available FIFO space was reserved by this task, its only writer. */
    uint32_t written = tud_cdc_n_write(PH_FW_LA_CDC, bytes, count * 4);
    sump_stream_index += written / 4;
    /* Full packets auto-flush in TinyUSB; flush a final short packet once. */
    if (sump_stream_index == sump_expected)
        tud_cdc_n_write_flush(PH_FW_LA_CDC);
}

static void handle_run(void)
{
    if (sump_req || sump_streaming || sump_done || ph_fw_la_busy())
        return; /* busy */
    /* SUMP dividers follow the OLS convention with a 100 MHz reference; the
     * libsigrok ols driver computes divider = Fclk/f - 1 with Fclk = 100 MHz,
     * so use that reference to keep the waveform time axis correct. */
    uint32_t hz = 100000000u / (reg_divider + 1u);
    uint8_t trigger = 0, pin = 0;
    if (reg_mask) {
        uint32_t bit = 0;
        while (bit < PH_FW_LA_CHANNELS && !(reg_mask & (1u << bit)))
            ++bit;
        if (bit == PH_FW_LA_CHANNELS)
            return; /* bad mask */
        trigger = (reg_value >> bit) & 1u ? 1u : 2u; /* rising : falling */
        pin = sump_dpin[bit];
    }
    uint32_t raw_mask = 0;
    for (uint32_t channel = 0; channel < PH_FW_LA_CHANNELS; ++channel)
        if (!reg_channels || (reg_channels & (1u << channel)))
            raw_mask |= 1u << sump_dpin[channel];
    if (trigger) raw_mask |= 1u << pin;
    uint32_t capacity = ph_fw_la_capacity(raw_mask, trigger);
    uint32_t quantum = trigger ? 4u : capacity / PH_FW_LA_SAMPLES;
    if (!capacity || !reg_count) return;
    uint32_t count = MIN(reg_count, capacity);
    count = ((count + quantum - 1u) / quantum) * quantum;
    sump_req_raw_mask = raw_mask;
    sump_packed = (reg_flags & 0x80000000u) != 0;
    uint32_t storage_bits = PH_FW_LA_SAMPLES * 32u / capacity;
    sump_expected_bytes = count * storage_bits / 8u;
    sump_crc = ~0u;
    sump_req_count = count;
    sump_req_hz = hz;
    sump_req_trigger = trigger;
    sump_req_pin = pin;
    sump_expected = count;
    __dmb();
    sump_req = 1;
    __dmb();
}

static void handle_reset(void)
{
    if (sump_streaming)
        sump_streaming = 0;
    __dmb();
    sump_cancel = 1;
    reg_divider = 0;
    reg_count = PH_FW_LA_SAMPLES; /* a full buffer by default */
    reg_flags = reg_channels = 0;
    reg_mask = reg_value = 0;
}

static void dispatch(const uint8_t *c, uint8_t len)
{
    uint8_t op = c[0];
    switch (op) {
    case 0x00:
        handle_reset();
        break;
    case 0x01:
        handle_run();
        break;
    case 0x02: {
        /* OLS demon-core identity accepted by libsigrok's ols driver. */
        ++id_pending;
        break;
    }
    case 0x84: /* pc1: completed-capture status, after the sample bytes. */
        ++status_pending;
        break;
    case 0x04: /* metadata: unsupported, stay silent */
        break;
    case 0x80:
        if (len == 5)
            reg_divider = le32(c + 1);
        break;
    case 0x81:
        if (len == 5) {
            uint32_t c4 = c[1] | ((uint32_t)c[2] << 8) | ((uint32_t)c[3] << 16);
            reg_count = c4 * 4u; /* Clamp against selected-channel capacity at Run. */
        }
        break;
    case 0x82:
        if (len == 5)
            reg_flags = le32(c + 1);
        break;
    case 0x83: /* pc1: selected SUMP channel mask; zero preserves all channels. */
        if (len == 5) reg_channels = le32(c + 1) & 0xffffu;
        break;
    case 0xc0:
    case 0xc4:
    case 0xc8:
    case 0xcc:
        if (len == 5 && (op & 0x0c) == 0)
            reg_mask = le32(c + 1);
        break;
    case 0xc1:
    case 0xc5:
    case 0xc9:
    case 0xcd:
        if (len == 5 && (op & 0x0c) == 0)
            reg_value = le32(c + 1);
        break;
    default:
        break;
    }
}

void ph_sump_init(void)
{
    reg_divider = 0;
    reg_count = PH_FW_LA_SAMPLES;
    reg_flags = reg_channels = 0;
    reg_mask = reg_value = 0;
    sump_req = sump_cancel = sump_done = 0;
    sump_streaming = 0;
    cmd_pos = cmd_need = 0;
    id_pending = status_pending = 0;
}

void ph_sump_pump(void)
{
    /* Parse commands from LA CDC. */
    while (tud_cdc_n_available(PH_FW_LA_CDC)) {
        uint8_t b;
        tud_cdc_n_read(PH_FW_LA_CDC, &b, 1);
        cmd_buf[cmd_pos++] = b;
        if (cmd_pos == 1)
            cmd_need = is_long(b) ? 5 : 1;
        if (cmd_pos >= cmd_need) {
            dispatch(cmd_buf, cmd_pos);
            cmd_pos = cmd_need = 0;
        }
    }

    if (id_pending && tud_cdc_n_write_available(PH_FW_LA_CDC) >= 4) {
        const uint8_t id[4] = {'1', 'A', 'L', 'S'};
        if (tud_cdc_n_write(PH_FW_LA_CDC, id, sizeof(id)) == sizeof(id))
            --id_pending;
        tud_cdc_n_write_flush(PH_FW_LA_CDC);
    }

    if (status_pending && !sump_done && !sump_streaming &&
        tud_cdc_n_write_available(PH_FW_LA_CDC) >= 20) {
        uint32_t status[5] = {0x31504350u, sump_flags, sump_actual, sump_sample_hz, ~sump_crc}; /* PCP1 */
        if (tud_cdc_n_write(PH_FW_LA_CDC, status, sizeof(status)) == sizeof(status))
            --status_pending;
        tud_cdc_n_write_flush(PH_FW_LA_CDC);
    }

    /* One packet per pump bounds Core 0 work so PH and DAP stay responsive. */
    if (sump_done) {
        __dmb();
        if (!sump_streaming) {
            sump_streaming = 1;
            sump_stream_index = 0;
        }
        if (tud_cdc_n_connected(PH_FW_LA_CDC))
            sump_stream();
        if (sump_stream_index >= (sump_packed ? sump_expected_bytes : sump_expected) || !tud_cdc_n_connected(PH_FW_LA_CDC)) {
            sump_streaming = 0;
            __dmb();
            sump_done = 0;
        }
    }
}

void ph_sump_worker(void)
{
    if (sump_cancel) {
        ph_fw_la_cancel();
        __dmb();
        sump_cancel = 0;
    }
    if (sump_req && !ph_fw_la_busy() && !sump_done) {
        __dmb();
        uint32_t count = sump_req_count, hz = sump_req_hz;
        uint8_t trigger = sump_req_trigger, pin = sump_req_pin;
        sump_req = 0;
        if (ph_fw_la_start_selected(count, hz, trigger, pin, PH_FW_TRIGGER_TIMEOUT_MS, sump_req_raw_mask,
                                  &sump_epoch, sump_epoch, &sump_lares)) {
            __dmb();
        }
    } else if (!sump_req && ph_fw_la_busy() && ph_fw_la_poll()) {
        /* Capture finished. Abort streaming on cancel; otherwise report the
         * captured sample count (0-padded in core0 if the trigger timed out). */
        if (!(sump_lares.flags & 8u)) {
            sump_actual = sump_lares.count;
            sump_flags = sump_lares.flags;
            sump_sample_hz = sump_lares.sample_hz;
            __dmb();
            sump_done = 1;
            __dmb();
        }
    }
}

void ph_sump_reset(void)
{
    sump_epoch++;
    sump_req = sump_cancel = sump_done = 0;
    sump_streaming = 0;
    cmd_pos = cmd_need = 0;
    id_pending = status_pending = 0;
}
