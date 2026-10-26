**English** · [简体中文](zh-CN/getting-started.md)

# Getting started with PicoHarness

See [ARCHITECTURE.md](ARCHITECTURE.md) for system structure, wiring and current interface limits. Connect USB to Debug; program the example firmware onto Work.

## 1. Build Debug and the desktop app

Install the official Pico SDK 2.x, ARM GCC, CMake, Ninja and Rust. On Linux, grant your user access to the serial and USB devices. Work programming and GDB require OpenOCD; Debug updates require picotool.

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
ph-bridge/target/release/phdcli daemon start
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

Debug exposes CMSIS-DAP, PH Terminal, PH Terminal Bulk and Logic Analyzer interfaces. `phd` owns device access; Python scripts and the desktop connect through TCP IPC. Add `ph-bridge/target/release` to PATH (use `.exe` on Windows). Python compatibility commands need only protobuf, with dependencies in `host/requirements.txt`.

```sh
phdcli device list
phdcli --serial SERIAL power on
phdcli --serial SERIAL status
phdcli --serial SERIAL measure
PYTHONPATH=host python3 -m phbridge -p SERIAL status
```

Use the discovered serial. Work starts powered off. Multiple probes are selected by serial consistently across control, Bulk, SWD and updates. `PHD_ADDR` and `PHD_TOKEN_FILE` select a remote daemon using the same clients. Native service installation, platform permissions, remote transport and GDB tunneling are documented in the [daemon guide](../ph-bridge/README.md); [MCP setup](../host/mcp/README.md) covers Claude and Codex.

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
PYTHONPATH=host python3 -m phbridge -p SERIAL console send 'help'
PYTHONPATH=host python3 -m phbridge -p SERIAL console read
PYTHONPATH=host python3 -m phbridge -p SERIAL term 460800
PYTHONPATH=host python3 -m phbridge -p SERIAL bulk /path/to/payload.bin
```

Short messages contain at most 128 bytes. The daemon retains Console events for independent readers; keep the returned cursor and instance, and handle `gap` as explicit history loss. Harness firmware announces its protocol when idle; wait for discovery before sending Bulk. Debug splits large files into SPI blocks of at most 2048 bytes. USB Bulk supports writes only, so large-file echo cannot verify readback. With plain UART firmware, configure Work to match any baud change yourself.

Use the separate Logic Analyzer CDC with a SUMP client, such as PulseView's `ols` driver; channel mapping is in the architecture document. With `harness-signals` running, observe GP5 at 10 kHz / 35% duty, UART on GP12/13 and SPI on GP18–21 during transfers. The buffer holds 32,768 samples, giving at most 16.384 ms at 2 MHz. SUMP implements a subset of the protocol; see the architecture document for limits on error and actual-rate reporting.

Desktop Logic uses phd’s dedicated SUMP worker. Restart an older phd after rebuilding; edge triggers require Debug firmware advertising `sump2`. GUI file Bulk remains unconnected; use the CLI path above.

## 5. Use GDB or your own application

Program Work from the desktop Firmware page, then import the Debug ELF from the **same build** on the GDB page. The daemon starts OpenOCD; the app creates an authenticated socket tunnel and attaches for breakpoints, Continue, Pause and stepping. **GDB server only** allows an external IDE to connect at the endpoint shown on the page. The daemon arbitrates exclusive device ownership during a managed session.

To add the Console library to your application, place this after `pico_sdk_init()`:

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

Call `pico_harness_init()` and keep calling `pico_harness_poll()` on the same core. Retry later when `pico_harness_write()` returns false. API contracts, resource usage and UART/SPI protocols are in [ARCHITECTURE.md](ARCHITECTURE.md). Plain applications can use SWD/GDB and passive capture without embedding this library.
