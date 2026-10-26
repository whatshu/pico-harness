/* SPDX-License-Identifier: MIT */
#include "pico_harness.h"
#include "pico/stdlib.h"
#include <string.h>

static uint8_t pending[2048];
static size_t pending_size;
static bool streaming;

static void receive(const uint8_t *data, size_t size, void *context)
{
    (void)context;
    size_t command_size = size;
    while (command_size && (data[command_size - 1] == '\r' || data[command_size - 1] == '\n')) {
        --command_size;
    }
    if (command_size == 9 && memcmp(data, "stream-on", 9) == 0) {
        streaming = true;
        return;
    }
    if (command_size == 10 && memcmp(data, "stream-off", 10) == 0) {
        streaming = false;
        return;
    }
    if (!pending_size && size <= sizeof(pending)) {
        memcpy(pending, data, size);
        pending_size = size;
    }
}

int main(void)
{
    pico_harness_init(receive, NULL);
    pico_harness_pwm(5, 10000, 0.35f);
    uint64_t next = 0;
    for (;;) {
        pico_harness_poll();
        if (pending_size && pico_harness_write(pending, pending_size)) {
            pending_size = 0;
        }
        uint64_t now = time_us_64();
        if (!pending_size && (streaming || now >= next) &&
            pico_harness_write("PicoHarness signals\r\n", 21)) {
            next = now + 20000;
        }
        tight_loop_contents();
    }
}
