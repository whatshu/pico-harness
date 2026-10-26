/* Exercise the production encoder and USB queues with short/blocked writes. */
#include <assert.h>
#include <string.h>
#include "hardware_stub.h"
uint64_t time_us_64(void);
uint32_t gpio_get_all(void);
bool gpio_get_dir(uint pin);
gpio_function_t gpio_get_function(uint pin);
bool gpio_is_pulled_up(uint pin);
bool gpio_is_pulled_down(uint pin);
#include "../src/ph_terminal.c"

static uint8_t usb[65536];
static size_t used;
static uint32_t budget;
uint64_t time_us_64(void) { return 123456; }
uint32_t time_us_32(void) { static uint32_t now; return ++now; }
bool tud_cdc_n_connected(uint8_t port) { assert(port == 0); return true; }
uint32_t tud_cdc_n_write_available(uint8_t port) { assert(port == 0); return budget; }
uint32_t tud_cdc_n_write_flush(uint8_t port) { assert(port == 0); return 0; }
uint32_t tud_cdc_n_write(uint8_t port, const void *data, uint32_t count)
{
    assert(port == 0);
    if (count > budget) count = budget;
    assert(used + count <= sizeof(usb));
    memcpy(usb + used, data, count); used += count; budget -= count;
    return count;
}
static void drain(void)
{
    unsigned iterations = 0;
    while (trace_head != trace_tail || tx_head != tx_tail || trace_left) {
        budget = 7; ph_flush_tx();
        assert(++iterations < 10000);
    }
}
static unsigned decode(unsigned *status, uint32_t *omitted)
{
    pb_istream_t input = pb_istream_from_buffer(usb, used);
    unsigned records = 0; *status = 0; *omitted = 0;
    while (input.bytes_left) {
        ph_msg_t msg = picoharness_v1_PhMessage_init_default;
        assert(pb_decode_delimited(&input, &picoharness_v1_PhMessage_msg, &msg));
        if (msg.seq == 42) { ++*status; continue; }
        assert(msg.which_body == picoharness_v1_PhMessage_uart_trace_tag);
        const picoharness_v1_UartTrace *trace = &msg.body.uart_trace;
        assert(trace->data.size == 128 && trace->baud == 115200);
        assert(trace->timestamp_us == 123456);
        for (unsigned i = 0; i < 128; ++i) assert(trace->data.bytes[i] == (uint8_t)i);
        if (trace->dropped > *omitted) *omitted = trace->dropped;
        ++records;
    }
    return records;
}
int main(void)
{
    uint8_t payload[128];
    for (unsigned i = 0; i < 128; ++i) payload[i] = i;
    ph_terminal_reset();
    for (unsigned i = 0; i < 30; ++i) ph_uart_trace_emit(false, 115200, payload, 128);
    assert(trace_omitted == 0); /* Bursts must not compete for 1 KiB control space. */
    budget = 7; ph_flush_tx(); /* Force a partial trace envelope. */
    assert(trace_left);
    ph_send_status(42, PH_STATUS_OK); /* Must follow the complete trace envelope. */
    drain();
    unsigned status; uint32_t omitted;
    assert(decode(&status, &omitted) == 30 && status == 1 && omitted == 0);

    ph_terminal_reset(); used = 0;
    for (unsigned i = 0; i < 1000; ++i) ph_uart_trace_emit(true, 115200, payload, 128);
    assert(trace_omitted > 0); /* Finite queues report loss without blocking control. */
    ph_send_status(42, PH_STATUS_OK);
    drain();
    ph_uart_trace_emit(true, 115200, payload, 128); drain();
    assert(decode(&status, &omitted) > 1 && status == 1 && omitted > 0);
    return 0;
}
