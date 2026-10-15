# PicoHarness Work Core library

[底座入门：构建、四个示例、Logic 和 GDB](GETTING_STARTED.md)

Use the Pico SDK's official `external/pico_sdk_import.cmake`, call
`pico_sdk_init()`, then add this directory and link `pico_harness`:

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

```c
#include "pico_harness.h"

static void receive(const uint8_t *data, size_t size, void *context)
{
    // Consume/copy data here. The pointer is valid only during this callback.
}

int main(void)
{
    pico_harness_init(receive, NULL);
    pico_harness_pwm(5, 10000, 0.35f);
    for (;;) {
        pico_harness_poll();
        // Retry pico_harness_write() later if it returns false.
        // Run your application's bounded work here.
    }
}
```

UART0 GP12/13 carries framed commands and messages up to 128 bytes. Larger
messages use the GP18/19/20/21 SPI slave and two DMA channels automatically;
each message is at most 2048 bytes. Split larger application messages into
chunks. The desktop joins received chunks when saving a file. `write()` copies
accepted data, returns immediately, and never allocates. Call `poll()` regularly
(about every 100 µs at 115200 baud, faster at higher rates); callbacks run on the
calling core. Use one core for this API and keep callbacks short. The library
uses one PIO0 state machine, 16 KiB of SPI DMA word buffers, bounded UART/bulk
queues, no interrupts or RTOS. PWM slices are shared between their A/B outputs;
choose an unused slice for `pico_harness_pwm()`.

Baud changes use an ACK at the old rate, a delayed switch, verification at the
new rate, and a commit. `write()` returns false during this transaction; retry
after `poll()` completes the commit or rollback. A prepare expires after 1 second
if the old-rate UART cannot drain; it cannot unexpectedly switch later. Work
restores its previous baud without a commit after
`max(1.5 s, 640 / actual_baud + 0.5 s)` (2.633334 s at 300 baud). At low baud rates,
queued messages can outlast the prepare deadline, so change baud while traffic
is idle. The desktop restores its endpoint on failure.

Examples are independent SDK projects that link `pico_harness`. Only projects
using the communication library belong in `example/`; standalone test firmware belongs in
`firmware/test/`, with a separate project directory for each fixture.

| Directory | Behavior |
| --- | --- |
| `example/harness-console` | Unified Console echo, UART and SPI bulk |
| `example/harness-terminal` | Console REPL: help/led/gpio/pwm/uart/adc/info/sleep/reboot |
| `example/harness-signals` | Console echo, periodic UART traffic, GP5 10 kHz / 35% PWM |

The plain [blink](../test/blink) and [UART echo](../test/uart-console)
fixtures live in `firmware/test/` and do not use this library.

`harness-terminal` is a MicroPython-style REPL over the harness console: `led
on|off|toggle` drives the board LED (GP25), `gpio` configures and drives or
reads any free pin, `pwm <pin> <hz> <duty>` uses `pico_harness_pwm()`, `uart
<hz>`/`uart tx <text>` drive a second UART (GP4 TX / GP5 RX), `adc <pin>`
reads GP26..28, and `info`/`sleep`/`reboot` cover the rest. Each Console send
is one command line; input is echoed and every result carries the `harness> `
prompt. `help` lists everything.

In `harness-signals`, send `stream-on` to fill the UART output continuously and
`stream-off` to restore the 20 ms interval. These commands accept optional CR/LF;
other messages are echoed. This exercises baud changes under sustained output.

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
cmake -S firmware/harness-target/example/harness-console -B /tmp/harness-console -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=Debug
cmake --build /tmp/harness-console -j
```

Upload the generated UF2 to **Work Core** in PicoHarness's Firmware page.
Plain UART projects support legacy Console traffic; coordinated baud changes
and SPI bulk require this library. `pico_harness_spi.h` is the shared engine for
hardware fixtures, not the normal application interface.

Run the portable protocol regressions without a Pico SDK or connected device:
`python3 firmware/harness-target/test/run.py` (requires a host C compiler).
These compile the production C against UART/clock/PWM stubs and check baud
backpressure and expiry, frame recovery, low-baud fragments, and PWM endpoints.
