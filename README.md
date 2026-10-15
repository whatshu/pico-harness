# PicoHarness

<p align="center"><a href="README.md"><strong>English</strong></a> · <a href="README.zh-CN.md">简体中文</a></p>

<p align="center">
  <img src="doc/image/front.png" alt="PicoHarness carrier board — front view" width="44%">
  <img src="doc/image/back.png" alt="PicoHarness carrier board — back view" width="33%">
</p>

PicoHarness turns **one ordinary Pico or Pico 2 dev board** into a full debug
probe for Pico/Pico 2 targets. Load the probe firmware onto a spare Pico, hook
up SWD and a UART pair to any RP2040/RP2350 board, and a single USB connection
gives you everything you need to bring up and debug Pico firmware — equally
drivable from the desktop app, a Python script, or an AI agent.

One USB connection provides:

- **CMSIS-DAP v2** — SWD to the target RP2040/RP2350: halt and resume,
  registers, GDB, memory read/write and export.
- **Console** — a two-way UART channel to the target; large payloads switch
  to SPI/DMA when the target runs the companion communication library.
- **Power control and metering** — switch the target rail and read real-time
  voltage, current and power (on the carrier or a custom harness).
- **Logic analyzer** — passive GPIO sampling while the target keeps running,
  with UART, SPI, I²C and PWM decoders.
- **Firmware upload** — program the target over SWD, or update the probe
  itself over USB.

The optional carrier board (`pcb/`) hosts the probe and adds target power
switching (TPS22919) plus INA237/INA226 sense metering. On the carrier the
probe is called **Debug Core** and the board being debugged **Work Core**;
nothing about the probe depends on the carrier.

Everything runs over the PDBG Link vendor-bulk protocol: little-endian,
CRC-checked frames carrying UART, SPI, power and logic-analyzer commands.

## Getting started

Prerequisites: the official Pico SDK (`PICO_SDK_PATH`), CMake, Ninja,
arm-none-eabi-gcc, and Rust for the desktop app. picotool is required for
probe updates; OpenOCD covers SWD/GDB and target uploads.

```sh
git submodule update --init --recursive
export PICO_SDK_PATH=/path/to/pico-sdk

./build.sh build            # probe UF2s -> RELEASE/artifacts/
./build.sh test --offline   # host, protocol and board offline checks
cargo test --manifest-path desktop/Cargo.toml --workspace

cargo run --manifest-path desktop/Cargo.toml -p picoharness-desktop
```

`./build.sh flash` upgrades the probe. `./build.sh test --hardware` runs a
target self-test and overwrites whatever is on the target flash, so back it up
first. With several probes attached, select one with `PDBG_SERIAL` or from the
device sidebar in the app.

Plug the carrier in (or wire probe SWIO/SWCLK and UART0 GP12/13 straight to the
target; the pin maps are in `doc/probe-firmware.md`), then follow
[doc/getting-started.md](doc/getting-started.md) for the four-step walkthrough.

## Documentation

All documentation is consolidated in `doc/`; the root README is the only
README.

| Doc | Contents |
| --- | --- |
| [getting-started.md](doc/getting-started.md) | First steps: build, flash and drive a target from the desktop |
| [desktop.md](doc/desktop.md) | Desktop app guide (Overview, Console, SWD/GDB, Logic, Firmware) |
| [desktop-testing.md](doc/desktop-testing.md) | UI performance tests and automated GUI tests |
| [work-library.md](doc/work-library.md) | `pico_harness` library: embed the console/SPI transport in your target firmware |
| [protocol.md](doc/protocol.md) | PDBG console wire protocol over UART/SPI |
| [probe-firmware.md](doc/probe-firmware.md) | Probe firmware: CMSIS-DAP + PDBG Link, board pin maps |
| [test-firmware.md](doc/test-firmware.md) | Standalone test fixtures (blink, UART, bandwidth, self-test) |
| [pcb.md](doc/pcb.md) | Carrier-board hardware notes |
| [probe-logic-analyzer.md](doc/probe-logic-analyzer.md) · [probe-upstream.md](doc/probe-upstream.md) | Third-party attribution and upstream provenance |

## Repository layout

| Path | Contents |
| --- | --- |
| `desktop/` | Rust/egui app (Overview, Console, SWD/GDB, Logic, Firmware) and the USB control library |
| `firmware/harness-probe/` | Probe firmware: CMSIS-DAP, PDBG Link, UART/SPI, GPIO sampling, power and metering |
| `firmware/harness-target/` | CMake-importable communication library and examples for the target Pico |
| `firmware/test/` | Standalone test firmware: blink, plain UART, bandwidth test, self-test |
| `host/` · `script/` | Python CLI (`pdbg`) and build/test entry points |
| `pcb/pico-dualcore-devboard-v1.2/` | Optional carrier board: EasyEDA project |
| `doc/` | Consolidated documentation (see above) |
| `third_party/` | Third-party components kept with their original licenses |

## License

Most of the project is MIT. The probe firmware is a combined build whose logic
analyzer is derived from
[gusmanb's LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer)
(GPL-3.0-only); the firmware that includes it is distributed under
GPL-3.0-only, while the desktop app and target communication library are MIT.
Third-party notices are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).