**English** · [简体中文](zh-CN/getting-started.md)

# Getting started with PicoHarness

See [ARCHITECTURE.md](ARCHITECTURE.md) for system structure, wiring and current interface limits. Connect USB to Debug; program the example firmware onto Work.

## 1. Build Debug and the desktop app

Install the official Pico SDK 2.x, ARM GCC, CMake, Ninja and Rust. On Linux, grant your user access to the serial and USB devices. Work programming and GDB require OpenOCD; Debug updates require picotool.

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build
cargo run --manifest-path desktop/Cargo.toml -p ph-app
```

The root script builds RP2040 Debug firmware at `RELEASE/artifacts/pico_dualcore_debug_pico.uf2`. For the first installation, hold Debug BOOTSEL while connecting USB, then copy the UF2. Once PH firmware is running, use `./build.sh flash`.

Build RP2350 Debug separately:

```sh
cmake -S firmware/harness-probe -B BUILD/harness-probe-pico2 -G Ninja \
  -DPICO_BOARD=pico_dualcore
cmake --build BUILD/harness-probe-pico2
```

## 2. Connect and control the board

Debug exposes CMSIS-DAP, PH Terminal, PH Terminal Bulk and Logic Analyzer interfaces. PH Terminal carries binary messages; access it through the CLI or a client library. The Python CLI requires `pyserial` and `protobuf`; Bulk also requires `pyusb`.

```sh
PYTHONPATH=host python3 -m phbridge device list
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 power on
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 status
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 measure
```

Replace the tty with the actual PH Terminal port. Work starts powered off; enable power before accessing it. With multiple probes, choose the control port explicitly. The Bulk CLI selects its USB data device automatically, so connect only one probe when using Bulk or CLI firmware updates.

## 3. Build and program a Work example

```sh
cmake -S firmware/harness-target/example/harness-terminal \
  -B BUILD/harness-terminal -G Ninja \
  -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=Debug
cmake --build BUILD/harness-terminal
```

For a Pico 2 Work board, use `PICO_BOARD=pico2` and a fresh build directory. Open the desktop **Firmware** page, select the actual Work chip and import the generated `harness_terminal.uf2`. Programming overwrites Work Flash.

| Example directory | Purpose |
| --- | --- |
| `firmware/test/blink` | Check programming and the LED; GP25 is not connected to carrier capture inputs |
| `firmware/test/uart-console` | Plain UART echo without the communication library, 115200 / 8N1 |
| `firmware/harness-target/example/harness-console` | Harness message echo |
| `firmware/harness-target/example/harness-terminal` | REPL; send `help` to list commands |
| `firmware/harness-target/example/harness-signals` | Periodic UART, GP5 PWM and `stream-on/off` for continuous output |

## 4. Use Console, Bulk and logic capture

```sh
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 console send 'help'
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 console read
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 term 460800
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 bulk /path/to/payload.bin
```

Short messages contain at most 128 bytes. Sending and reading are separate CLI operations; the sending process may already consume output, so interactive applications should keep one client connection open. Harness firmware announces its protocol when idle; wait for discovery before sending Bulk. Debug splits large files into SPI blocks of at most 2048 bytes. USB Bulk supports writes only, so large-file echo cannot verify readback. With plain UART firmware, configure Work to match any baud change yourself.

Use the separate Logic Analyzer CDC with a SUMP client, such as PulseView's `ols` driver; channel mapping is in the architecture document. With `harness-signals` running, observe GP5 at 10 kHz / 35% duty, UART on GP12/13 and SPI on GP18–21 during transfers. The buffer holds 32,768 samples, giving at most 16.384 ms at 2 MHz. SUMP implements a subset of the protocol; see the architecture document for limits on error and actual-rate reporting.

The desktop Logic capture and file Bulk paths are not connected to the dedicated interfaces. Use the CLI / SUMP paths above.

## 5. Use GDB or your own application

Program Work from the desktop Firmware page, then import the Debug ELF from the **same build** on the GDB page. The app starts OpenOCD and attaches for breakpoints, Continue, Pause and stepping. **GDB server only** allows an external IDE to connect at the endpoint shown on the page. Live releases USB during a managed session.

To add the Console library to your application, place this after `pico_sdk_init()`:

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

Call `pico_harness_init()` and keep calling `pico_harness_poll()` on the same core. Retry later when `pico_harness_write()` returns false. API contracts, resource usage and UART/SPI protocols are in [ARCHITECTURE.md](ARCHITECTURE.md). Plain applications can use SWD/GDB and passive capture without embedding this library.
