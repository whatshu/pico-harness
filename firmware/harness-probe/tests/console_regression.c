/* Production console with a deterministic UART boundary. No protocol reconstruction
 * is allowed in the trace: malformed RX and partial TX must survive verbatim. */
#include <assert.h>
#include <string.h>
#include "ph_console.h"
#include "ph_terminal.h"

static uint8_t input[256], trace_bytes[256], output[256];
static size_t input_size, trace_size, output_size, write_limit = 256;
static unsigned trace_count, console_count;
static bool trace_tx;
static uint32_t trace_baud;
static uint64_t now;
uint64_t time_us_64(void) { return now; }
void busy_wait_until(uint64_t when) { now = when; }
void tight_loop_contents(void) { now += 1000; }
uint32_t clock_get_hz(unsigned clock) { (void)clock; return 125000000; }
bool ph_fw_target_io_ready(void) { return true; }
bool ph_fw_target_serial_config(uint8_t channel, uint32_t baud, uint8_t bits,
                               uint8_t stop, uint8_t parity, uint32_t *actual)
{ (void)channel; (void)bits; (void)stop; (void)parity; *actual = baud; return true; }
size_t ph_fw_target_serial_write(uint8_t channel, const uint8_t *data, size_t count)
{
    (void)channel;
    if (count > write_limit) count = write_limit;
    memcpy(output, data, count); output_size = count;
    return count;
}
size_t ph_fw_target_serial_read(uint8_t channel, uint8_t *data, size_t count)
{
    (void)channel;
    if (count > input_size) count = input_size;
    memcpy(data, input, count);
    memmove(input, input + count, input_size - count); input_size -= count;
    return count;
}
void ph_uart_trace_emit(bool tx, uint32_t baud, const uint8_t *data, size_t count)
{
    if (!count) return;
    assert(trace_size + count <= sizeof(trace_bytes));
    memcpy(trace_bytes + trace_size, data, count); trace_size += count;
    trace_tx = tx; trace_baud = baud; ++trace_count;
}
void ph_console_data_emit(const uint8_t *data, size_t count)
{ (void)data; (void)count; ++console_count; }
void ph_bulk_on_uart(uint8_t kind, uint16_t seq, const uint8_t *data, size_t count)
{ (void)kind; (void)seq; (void)data; (void)count; }
uint16_t ph_fw_crc16_ccitt(const uint8_t *data, size_t count)
{
    uint16_t crc = 0xffff;
    while (count--) {
        crc ^= (uint16_t)*data++ << 8;
        for (unsigned i = 0; i < 8; ++i)
            crc = (crc << 1) ^ ((crc & 0x8000) ? 0x1021 : 0);
    }
    return crc;
}
static void clear_trace(void) { trace_size = trace_count = 0; }
int main(void)
{
    const uint8_t hello[] = {0x50,0x48,1,0x81,0,0,4,0,1,7,0,8,0x60,0xae};
    ph_console_init();
    memcpy(input, hello, sizeof(hello)); input_size = sizeof(hello);
    ph_console_pump();
    assert(ph_console_mode() == PH_CONSOLE_HARNESS);
    assert(trace_size == sizeof(hello) && !trace_tx && trace_baud == 115200);
    assert(!memcmp(trace_bytes, hello, sizeof(hello)));
    clear_trace(); console_count = 0;
    memcpy(input, hello, sizeof(hello)); input_size = sizeof(hello);
    ph_console_pump();
    assert(console_count == 0 && trace_size == sizeof(hello));
    clear_trace();
    const uint8_t invalid[] = {0xff, 0, 'P', 'H', 99, 0x83, 0, 1, 0, 0, 0xee};
    memcpy(input, invalid, sizeof(invalid)); input_size = sizeof(invalid);
    ph_console_pump();
    assert(!trace_tx && trace_size == sizeof(invalid));
    assert(!memcmp(trace_bytes, invalid, sizeof(invalid)));
    clear_trace();
    write_limit = 5;
    ph_console_write((const uint8_t *)"test", 4);
    assert(trace_tx && trace_size == 5 && output_size == 5);
    assert(!memcmp(trace_bytes, output, 5));
    assert(trace_bytes[0] == 'P' && trace_bytes[1] == 'H' && trace_bytes[3] == 2);
    clear_trace();
    ph_console_reset();
    ph_console_write((const uint8_t *)"plain", 5);
    assert(trace_tx && trace_size == 5 && !memcmp(trace_bytes, "plain", 5));
    return 0;
}
