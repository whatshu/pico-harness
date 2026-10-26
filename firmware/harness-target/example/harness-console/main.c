/* SPDX-License-Identifier: MIT */
#include "pico_harness.h"
#include "pico/stdlib.h"
#include <string.h>

static uint8_t pending[2048];
static size_t pending_size;

static void receive(const uint8_t *data, size_t size, void *context)
{
    (void)context;
    if (!pending_size && size <= sizeof(pending)) {
        memcpy(pending, data, size);
        pending_size = size;
    }
}

int main(void)
{
    pico_harness_init(receive, NULL);
    for (;;) {
        pico_harness_poll();
        if (pending_size && pico_harness_write(pending, pending_size)) {
            pending_size = 0;
        }
        tight_loop_contents();
    }
}
