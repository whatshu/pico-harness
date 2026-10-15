# PicoHarness getting started

The four examples confirm, in order: powering up, watching plain UART, using
the PicoHarness Console, and capturing a running UART / SPI / PWM. First attach
the Debug Core to the host and confirm the Desktop sees the device; the target
of every example flash is always the **Work Core**.

`blink` and `uart-console` (no communication library) live in
`firmware/test/`, each an independent test project; the communication-library examples
`harness-console` and `harness-signals` live in `firmware/harness-target/example/`.

## 1. Build and flash

Install the Pico SDK, ARM GCC, CMake and OpenOCD. Every example is a
standalone SDK project using the official `external/pico_sdk_import.cmake`;
none depends on this repository's top-level build.

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
cmake -S firmware/test/blink -B build/blink \
    -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=Debug
cmake --build build/blink -j
```

Drag `build/blink/blink.uf2` into the Desktop **Firmware** page. The app turns
on Work power, validates the image, and flashes it. Later examples just
replace the source directory and build directory.

Examples default to Debug builds that keep source and variable information;
use `-DCMAKE_BUILD_TYPE=Release` for release firmware. A plain RP2040 Pico sets
`PICO_BOARD=pico`; create a fresh build directory when changing boards. The
sample rates below were measured on an RP2040.

## 2. Run the examples in order

| Example | What the firmware does | Desktop actions and expected results |
| --- | --- | --- |
| `blink` | Toggles the on-board LED (GP25) every 250 ms, no communication library | LED blinks; Overview Live shows real power, voltage, current. The pause icon freezes the display while measurements and any enabled recording continue. GP25 is not wired to the carrier sampling net, so Logic shows no LED waveform. |
| `uart-console` | UART0, 115200 / 8N1, echoes byte by byte | Type text into Console and send; see the echo. The status shows plain UART with no confirmed protocol support. This example has no coordinated-baud or SPI-bulk capability; changing baud means editing the example and re-flashing. |
| `harness-console` | Echoes complete messages through the `pico_harness` library | Console status should change to protocol-verified. Short messages go over UART; messages over 128 bytes are automatically chunked over SPI. Use "send file" with a binary, then "save received data" and compare. Apply coordinates the baud switch, verifies it, and rolls back on failure. |
| `harness-signals` | Console echo, one UART message every 20 ms, plus a 10 kHz / 35% PWM on GP5 | For the Logic and GDB exercises below. SPI clock activity appears only while sending a file. `stream-on` enables continuous UART output; `stream-off` returns to periodic output. |

Console Up/Down recalls sent history; returning to the end restores your
unsent draft. "Transport trace" read-only shows the raw UART/SPI data and
supports export. Console's "pause scroll" only holds the scroll position; it
differs from Live's pause-refresh.

Console baud has eight steps: 9600, 19200, 38400, 57600, 115200, 230400,
460800, 921600. Pick one and Apply; only communication-library firmware
supports coordinated switching. Transport trace shows the protocol commands,
sequence numbers, parameters and CRC, and the CSV export includes both raw
bytes and decoded explanations.

The UART protocol declares UART, SPI bulk and coordinated baud switching,
**but not PWM state**. Real PWM frequency and duty are read through Logic
capture, not inferred from the protocol handshake.

## 3. Capture signals and add decoders

Flash `harness-signals` and open **Logic**:

1. Choose **PicoHarness defaults** in Channels. That selects GP5, GP12, GP13,
   GP18, GP19, GP20, GP21 and adds matching PWM, UART and SPI decoders.
   Physical channels always stay in GP numeric order; defaults carry plain
   `GPn` names.
2. Pick **2 MHz** and **16 ms** and click Run. Clicking a channel label
   renames it, sets its trigger, or adds a decoder. Decoders appear as
   separate virtual channels; click their labels to configure the input GPIO,
   rate and protocol parameters. Decoder settings only change how the
   already-captured samples are interpreted; they never modify hardware.
3. GP5 should show an ≈10 kHz, 35% PWM. The UART decoder uses the Console's
   current actual baud; after changing the Console baud, re-select defaults or
   adjust the decoder parameters.
4. Choose a rising-edge trigger on GP5 and Run again. The first sample
   corresponds to the rising edge; the falling edge works the same way. No
   matching edge shows a trigger timeout; change the channel or increase the
   wait time. Stop cancels the wait, and later captures still work.
5. To watch SPI, start the capture and immediately send a large file from
   Console. Capture is passive input; UART / SPI / PWM keep running. SPI uses
   CLK=GP18, MOSI=GP20, MISO=GP19, CS=GP21, mode 0.

Supported rates are **100 kHz, 500 kHz, 1 MHz, 2 MHz, 5 MHz, 10 MHz, 20 MHz**.
The buffer holds at most 32,768 samples, so 2 MHz gives up to 16.384 ms and
100 kHz up to 327.68 ms; higher rates shorten the longest window. Desktop
Console's SPI bulk clock is 1 MHz, about 20 sample points per clock at the
20 MHz setting; raise the capture rate accordingly when you use a faster SPI
clock yourself.

All / None select or clear the 16 GPIOs the carrier actually connects;
unconnected GPIOs are not shown. Adding or changing a decoder input
auto-selects the associated channels. Export supports raw CSV, VCD and decoded
CSV.

### Verifying Logic without the communication library

`GPn` is always a **Work Core target pin**. The carrier connects GP5,
GP10–21 and GP26–28; GP25 is the Pico's on-board LED, which is not on the
main header and has no carrier sampling connection. It is therefore expected
that raw `blink`'s observable channels stay flat. To capture your own
program's toggling, flip GP5 in your blink code too, or use `harness-signals`'
GP5 PWM.

Plain UART waveforms can be verified without touching blink: type a test string
into Console, switch to Logic, select GP13, 2 MHz, rising edge, a 10 s
timeout, Run, then switch back to Console and send. GP13 is host → Work and
GP12 is Work → host; blink does not echo, so GP12 does not trigger on this
send. Add a UART decoder to GP13 at 115200 / 8N1. This path sends plain UART
bytes and does not require the Harness protocol.

Trigger wait time is not the capture window: 10 s waits for an edge, while
2 MHz / 32,768 points record only the 16.384 ms after the trigger. Work must
be running; if GDB paused the target, Continue or disconnect and use the
top-level Reset.

## 4. View memory and debug source

**SWD**: connect Core 0, Halt, then enter `0x20000000` in Memory and press
Enter / Refresh. Addresses accept decimal or `0x`/`0X` hex; plain views align
to 4 bytes. Export a range with a start address, length, and Binary / Intel
HEX / CSV. After Resume, PC, SP, etc. keep their last snapshot, shown grey as
cached values. After an invalid memory-access error you can still reach valid
addresses and Resume.

**GDB**: first flash the current Debug-build UF2 in Firmware, then switch to
the GDB page and drop in the ELF from the **same build**. The app starts a
fresh OpenOCD and connects; dropping the ELF starts debugging and does not
auto-flash it (dropping an ELF on the Firmware page still means flashing).
Tick Follow PC, click a line number to set a breakpoint, and use Continue,
Pause, Step over / into / out. GDB picks SRAM software breakpoints or Flash
hardware breakpoints from the memory map; hardware breakpoints are limited.

GDB's Memory uses the edited Address; Read, Enter and the top Refresh read
that address, and Write accepts HEX bytes and reads them back after writing.
Reading near the end of SRAM clamps to the valid boundary and shows the actual
length: after RP2040 `0x20041FD0` only 48 RAM bytes remain, so a 64-byte read
cannot complete.

To use VS Code instead, choose **GDB server only** and connect — no ELF
needed. The page shows the actual listening address; configure Cortex-Debug
with `servertype: external`, `gdbTarget` and `request: attach`, plus a
matching local ELF and GDB path. Or run directly:

```sh
gdb-multiarch build/harness-signals/harness_signals.elf \
    -ex 'target extended-remote 127.0.0.1:3333'
```

Replace 3333 with the port the page shows. Closing the desktop connection
closes the OpenOCD it started; external editors should disconnect first.
During debugging, Live releases USB so OpenOCD can own the probe.

The RP2040 server the Desktop starts defaults to core0, keeping another paused
core from freezing the shared timer and blocking UART initialization. If an
earlier external dual-core server left core1 halted, disconnect that server,
click Work Reset at the top, and start a new session. When connecting to an
existing external server, its configuration is kept.

## 5. Use it in your own project

Import the library only if you want the UART/SPI Console or coordinated baud
changes. Plain applications can still be debugged through SWD/GDB and Logic
without embedding any communication code.

```cmake
# After the official pico_sdk_init()
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

The library takes over the Work's UART0 GP12/13, SPI GP18–21, one PIO0 state
machine and two DMA channels. PWM GP5 is a choice made by the signals example;
the library does not reserve it automatically. Call `pico_harness_poll()` to
keep the queue and protocol moving, keep callbacks short, and retry
`pico_harness_write()` later when it returns false. The API, queue sizes and
performance limits are in [README](README.md).

Reproducible hardware-sampling checks (run `harness-signals` first and keep
Work powered):

```sh
python3 -m host.tools.check_logic_presets --serial YOUR_PROBE_SERIAL \
    --output /tmp/picoharness-logic.json
```

This command flashes nothing and leaves power untouched; it verifies all seven
rates, the three trigger modes, and recovery after timeout and cancel.

With `harness-signals` flashed and a matching ELF, GDB initialization, Flash
breakpoints and source stepping can be verified:

```sh
cargo run --manifest-path desktop/Cargo.toml -p picoharness-desktop \
    --bin gdb_smoke -- --flash-fixture build/harness-signals/harness_signals.elf \
    YOUR_PROBE_SERIAL
```

The check resets, pauses and resumes Work without downloading or modifying
Flash.