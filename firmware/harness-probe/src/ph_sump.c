/* SPDX-License-Identifier: MIT */
#include <string.h>

#include "hardware/clocks.h"
#include "hardware/sync.h"
#include "pico/stdlib.h"
#include "tusb.h"

#include "ph_fw_la.h"
#include "ph_sump.h"
#include "ph_fw_target.h"

#define LA_CDC 1u
#define LA_CHANNELS 16u
#define TRIGGER_TIMEOUT_MS 5000u

/* SUMP channel -> work-pin bit position (the same PH_FW_GPIO_MAP order the
 * capture engine uses for `ph_fw_la_sample`'s normalized words). */
#define PH_SUMP_WPIN(debug_pin, work_pin) work_pin,
static const uint8_t sump_wpin[LA_CHANNELS] = { PH_FW_GPIO_MAP(PH_SUMP_WPIN) };

/* SUMP registers. */
static uint32_t reg_divider, reg_count, reg_flags;
static uint32_t reg_mask, reg_value; /* stage-0 trigger mask/value */

/* core0 <-> core1 mailbox. */
static volatile uint8_t sump_req, sump_cancel, sump_done;
static volatile uint32_t sump_req_count, sump_req_hz;
static volatile uint8_t sump_req_trigger, sump_req_pin;
static volatile uint32_t sump_actual;
static volatile uint32_t sump_epoch /* volatile for ph_fw_la_start */ ;
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
    return op == 0x80 || op == 0x81 || op == 0x82 || (op >= 0xc0 && op <= 0xcf);
}

static uint32_t le32(const uint8_t *b)
{
    return (uint32_t)b[0] | ((uint32_t)b[1] << 8) | ((uint32_t)b[2] << 16) |
           ((uint32_t)b[3] << 24);
}

static void cdc1_write(const uint8_t *data, size_t len)
{
    size_t off = 0;
    while (off < len) {
        uint32_t n = tud_cdc_n_write(LA_CDC, data + off, len - off);
        off += n;
        if (n == 0) {
            tight_loop_contents();
            if (!tud_cdc_n_connected(LA_CDC))
                break;
        }
    }
    /* Do not flush here: flushing per ID reply would stall bulk streaming.
     * A single 4-byte ID will be delivered when the host reads it. */
    tud_cdc_n_write_flush(LA_CDC);
}

static void sump_stream_sample(uint32_t idx)
{
    uint8_t word[4];
    uint32_t s = idx < sump_actual ? ph_fw_la_sample(idx) : 0;
    uint32_t w = 0;
    for (uint32_t i = 0; i < LA_CHANNELS; ++i)
        w |= ((s >> sump_wpin[i]) & 1u) << i;
    word[0] = (uint8_t)w;
    word[1] = (uint8_t)(w >> 8);
    word[2] = (uint8_t)(w >> 16);
    word[3] = (uint8_t)(w >> 24);
    cdc1_write(word, 4);
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
        while (bit < LA_CHANNELS && !(reg_mask & (1u << bit)))
            ++bit;
        if (bit == LA_CHANNELS)
            return; /* bad mask */
        trigger = (reg_value >> bit) & 1u ? 1u : 2u; /* rising : falling */
        pin = (uint8_t)sump_wpin[bit];
    }
    uint32_t count = reg_count;
    if (trigger && count < 4)
        count = 4;
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
    reg_count = PH_FW_LA_SAMPLES / 4; /* a full buffer by default */
    reg_flags = 0;
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
        const uint8_t id[4] = {'1', 'A', 'L', 'S'};
        cdc1_write(id, sizeof(id));
        break;
    }
    case 0x04: /* metadata: unsupported, stay silent */
        break;
    case 0x80:
        if (len == 5)
            reg_divider = le32(c + 1);
        break;
    case 0x81:
        if (len == 5) {
            uint32_t c4 = c[1] | ((uint32_t)c[2] << 8) | ((uint32_t)c[3] << 16);
            reg_count = c4 * 4u > PH_FW_LA_SAMPLES ? PH_FW_LA_SAMPLES : c4 * 4u;
        }
        break;
    case 0x82:
        if (len == 5)
            reg_flags = le32(c + 1);
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
    reg_count = PH_FW_LA_SAMPLES / 4;
    reg_flags = 0;
    reg_mask = reg_value = 0;
    sump_req = sump_cancel = sump_done = 0;
    sump_streaming = 0;
    cmd_pos = cmd_need = 0;
}

void ph_sump_pump(void)
{
    /* Parse commands from LA CDC. */
    while (tud_cdc_n_available(LA_CDC)) {
        uint8_t b;
        tud_cdc_n_read(LA_CDC, &b, 1);
        cmd_buf[cmd_pos++] = b;
        if (cmd_pos == 1)
            cmd_need = is_long(b) ? 5 : 1;
        if (cmd_pos >= cmd_need) {
            dispatch(cmd_buf, cmd_pos);
            cmd_pos = cmd_need = 0;
        }
    }

    /* Stream a completed capture out of the shared buffer. */
    if (sump_done) {
        if (!sump_streaming) {
            sump_streaming = 1;
            sump_stream_index = 0;
        }
        while (sump_stream_index < sump_expected) {
            if (!tud_cdc_n_connected(LA_CDC)) {
                sump_streaming = 0;
                break;
            }
            if (tud_cdc_n_write_available(LA_CDC) >= 4)
                sump_stream_sample(sump_stream_index++);
            else
                break;
            tight_loop_contents();
        }
        if (sump_stream_index >= sump_expected || !sump_streaming) {
            sump_done = 0;
            sump_streaming = 0;
            tud_cdc_n_write_flush(LA_CDC);
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
        uint32_t count = sump_req_count, hz = sump_req_hz;
        uint8_t trigger = sump_req_trigger, pin = sump_req_pin;
        sump_req = 0;
        if (ph_fw_la_start(count, hz, trigger, pin, TRIGGER_TIMEOUT_MS, false, &sump_epoch,
                     sump_epoch, &sump_lares)) {
            __dmb();
        }
    } else if (!sump_req && ph_fw_la_busy() && ph_fw_la_poll()) {
        /* Capture finished. Abort streaming on cancel; otherwise report the
         * captured sample count (0-padded in core0 if the trigger timed out). */
        if (!(sump_lares.flags & 8u)) {
            sump_actual = sump_lares.count;
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
}