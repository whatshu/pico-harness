/* Host-side USB/UART/LA adapter for the production Bulk and SUMP state machines. */
#include <assert.h>
#include <string.h>
#include "hardware_stub.h"
#include "ph_console.h"
#include "ph_fw_la.h"
#include "ph_terminal.h"

uint8_t mock_vendor_input[8192], mock_cdc_input[128], mock_cdc_output[131072];
unsigned mock_vendor_size, mock_vendor_pos, mock_cdc_size, mock_cdc_pos;
unsigned mock_cdc_used, mock_cdc_space, mock_cdc_writes, mock_cdc_flushes;
unsigned mock_done_count, mock_done_ok, mock_done_crc;
unsigned mock_uart_kind, mock_uart_length, mock_la_active, mock_la_complete;
unsigned mock_la_count, mock_la_cancels;
static ph_fw_la_result_t *la_result;

void mock_transport_reset(void)
{
    mock_vendor_size = mock_vendor_pos = mock_cdc_size = mock_cdc_pos = 0;
    mock_cdc_used = mock_cdc_writes = mock_cdc_flushes = 0;
    mock_cdc_space = 64;
    mock_done_count = mock_done_ok = mock_done_crc = 0;
    mock_uart_kind = mock_uart_length = mock_la_active = mock_la_complete = 0;
    mock_la_count = mock_la_cancels = 0;
}

uint32_t tud_vendor_read(void *buffer, uint32_t count)
{
    count = MIN(count, mock_vendor_size - mock_vendor_pos);
    memcpy(buffer, mock_vendor_input + mock_vendor_pos, count);
    mock_vendor_pos += count;
    return count;
}

uint32_t tud_cdc_n_available(uint8_t port)
{
    assert(port == 1);
    return mock_cdc_size - mock_cdc_pos;
}

uint32_t tud_cdc_n_read(uint8_t port, void *buffer, uint32_t count)
{
    count = MIN(count, tud_cdc_n_available(port));
    memcpy(buffer, mock_cdc_input + mock_cdc_pos, count);
    mock_cdc_pos += count;
    return count;
}

uint32_t tud_cdc_n_write_available(uint8_t port)
{
    assert(port == 1);
    return mock_cdc_space;
}

uint32_t tud_cdc_n_write(uint8_t port, const void *buffer, uint32_t count)
{
    count = MIN(count, tud_cdc_n_write_available(port));
    assert(mock_cdc_used + count <= sizeof(mock_cdc_output));
    memcpy(mock_cdc_output + mock_cdc_used, buffer, count);
    mock_cdc_used += count;
    mock_cdc_space -= count;
    ++mock_cdc_writes;
    return count;
}

uint32_t tud_cdc_n_write_flush(uint8_t port)
{
    assert(port == 1);
    ++mock_cdc_flushes;
    return 0;
}

bool tud_cdc_n_connected(uint8_t port)
{
    assert(port == 1);
    return true;
}

ph_console_mode_t ph_console_mode(void)
{
    return PH_CONSOLE_HARNESS;
}

void ph_console_uart_send(uint8_t kind, const uint8_t *payload, size_t size)
{
    assert(size == 2);
    mock_uart_kind = kind;
    mock_uart_length = payload[0] | (payload[1] << 8);
}

void ph_send_body(uint32_t seq, ph_status_t status, pb_size_t tag,
                  const void *body, size_t size)
{
    assert(seq == 0 && status == PH_STATUS_OK);
    assert(tag == picoharness_v1_PhMessage_bulk_tx_done_tag);
    assert(size == sizeof(ph_bulk_tx_done_t));
    const ph_bulk_tx_done_t *done = body;
    ++mock_done_count;
    mock_done_ok = done->ok;
    mock_done_crc = done->crc_recv;
}

bool ph_fw_la_start(uint32_t count, uint32_t hz, uint8_t trigger, uint8_t pin,
                    uint16_t timeout, bool bypass, const volatile uint32_t *epoch,
                    uint32_t expected, ph_fw_la_result_t *result)
{
    (void)hz; (void)trigger; (void)pin; (void)timeout; (void)bypass;
    (void)epoch; (void)expected;
    assert(!mock_la_active);
    mock_la_count = count;
    mock_la_active = 1;
    la_result = result;
    return true;
}

bool ph_fw_la_busy(void)
{
    return mock_la_active;
}

bool ph_fw_la_poll(void)
{
    if (!mock_la_active || !mock_la_complete)
        return false;
    la_result->count = mock_la_count;
    la_result->flags = 0;
    mock_la_active = 0;
    return true;
}

void ph_fw_la_cancel(void)
{
    ++mock_la_cancels;
    mock_la_active = 0;
}

uint32_t ph_fw_la_sample(uint32_t index)
{
    /* Every sample differs, exercising channel order and stream offsets. */
    return (index * 0x12345u) ^ 0x1c3ffc20u;
}
