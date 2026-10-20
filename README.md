**English** · [简体中文](doc/zh-CN/README.md)

# PicoHarness

<p align="center">
  <img src="doc/image/front.png" alt="PicoHarness carrier board — front view" width="44%">
  <img src="doc/image/back.png" alt="PicoHarness carrier board — back view" width="33%">
</p>

PicoHarness turns a Pico or Pico 2 into a USB debug probe for RP2040/RP2350
targets. On the optional carrier, the probe is **Debug Core** and the target
is **Work Core**. The carrier adds target power switching and INA237/INA226
metering; custom wiring requires matching board definitions.

One USB connection exposes four separate functions:

- **CMSIS-DAP v2** for SWD, OpenOCD/GDB, memory access and target programming.
- **PH Terminal** for protobuf control messages, measurements and short UART Console data.
- **PH Terminal Bulk** for raw host-to-target payloads forwarded over SPI/DMA.
- **Logic Analyzer CDC** for passive PIO/DMA captures through a SUMP client.

The Rust/egui desktop provides Overview, Console, SWD, GDB, Logic and Firmware
pages. Its SPI/Bulk and capture hardware paths are not connected to the dedicated
interfaces; use the PH CLI for Bulk writes and a SUMP client for acquisition.
USB Bulk readback is not implemented. Interface contracts and remaining
implementation limits are documented in [ARCHITECTURE.md](doc/ARCHITECTURE.md).

## Build and run

Requires Pico SDK 2.x (`PICO_SDK_PATH`), ARM GCC, CMake, Ninja and Rust.
OpenOCD handles Work programming/debugging; picotool handles Debug updates.

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
ph-bridge/target/release/phdcli daemon start
cargo run --manifest-path desktop/Cargo.toml -p ph-app
```

The root build packages RP2040 Debug firmware in `RELEASE/artifacts/`.
Pico 2 Debug builds, wiring, Work examples and CLI commands are covered in the
[getting started guide](doc/getting-started.md). Test commands and hardware
requirements are in [desktop-testing.md](doc/desktop-testing.md).

The desktop and host scripts use `phd` socket IPC. The daemon owns USB and
programmer processes. It supports local and remote control, independent Console
history and exclusive debug/programming sessions. See the [daemon guide](ph-bridge/README.md)
and [Claude/Codex MCP setup](host/mcp/README.md).

## Documentation and source

| Document | Contents |
| --- | --- |
| [ARCHITECTURE.md](doc/ARCHITECTURE.md) | System structure, pins, USB/PH and Work protocols, firmware, libraries, desktop boundaries and builds |
| [getting-started.md](doc/getting-started.md) | Build, connect, program and use the available interfaces |
| [desktop-testing.md](doc/desktop-testing.md) | Offline, GUI, native performance and hardware checks |
| [probe-upstream.md](doc/probe-upstream.md) · [probe-logic-analyzer.md](doc/probe-logic-analyzer.md) | Source provenance and license attribution |

Source is organized under `firmware/` (Debug, Work library and fixtures),
`ph-bridge/` (Rust daemon, CLI, socket client, hardware modules and schemas),
`host/` (Python scripts and MCP), `desktop/` (GUI), `pcb/` (carrier), `script/` and `third_party/`.

## License

Original project code is generally MIT. The combined Debug firmware includes
[gusmanb's LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer) sampler and
is distributed under GPL-3.0-only; the desktop, host clients and Work library
retain their applicable licenses. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
