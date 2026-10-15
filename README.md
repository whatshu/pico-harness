# PicoHarness

<p align="center"><a href="README.md"><strong>English</strong></a> · <a href="README.zh-CN.md">简体中文</a></p>

<p align="center">
  <img src="doc/image/front.png" alt="PicoHarness carrier board — front view" width="44%">
  <img src="doc/image/back.png" alt="PicoHarness carrier board — back view" width="33%">
</p>

A debug and measurement toolkit for a two-Pico carrier board. The Debug Core is
a CMSIS-DAP probe that switches the Work Pico's power with a TPS22919 gate,
drives its RUN and BOOTSEL lines, and reads voltage, current and power from an
INA237/INA226 sense resistor. Everything it does — SWD, UART, SPI, GPIO
sampling, power, firmware flashing — is exposed over USB through one small
binary protocol, so the board can be driven from the desktop app, from a
script, or from an AI agent with equal ease.

One USB connection gives you:

- **CMSIS-DAP v2** — SWD to the Work core: halt and resume, registers, GDB,
  memory read/write and export.
- **Power control and metering** — switch the Work rail on and off, and read
  real-time voltage, current and power from the sense branch.
- **Console** — a two-way UART channel to the Work core; large payloads switch
  to SPI/DMA when the Work side runs the companion communication library.
- **Logic analyzer** — passive GPIO sampling while the system keeps running,
  with UART, SPI, I²C and PWM decoders.
- **Firmware upload** — program the Work core over SWD, or update the Debug
  Core itself over USB.

Communication runs over the PDBG Link vendor-bulk protocol: little-endian,
CRC-checked frames carrying UART, SPI, power and logic-analyzer commands.

## Repository layout

| Path | Contents |
| --- | --- |
| `desktop/` | Rust/egui app (Overview, Console, SWD/GDB, Logic, Firmware) and the USB control library |
| `firmware/harness-probe/` | Debug Core firmware: CMSIS-DAP, PDBG Link, UART/SPI, GPIO sampling, power and metering |
| `firmware/harness-target/` | CMake-importable communication library and examples for the Work Pico |
| `firmware/test/` | Standalone test firmware: blink, plain UART, bandwidth test, Work self-test |
| `host/` · `script/` | Python CLI (`pdbg`) and build/test entry points |
| `pcb/pico-dualcore-devboard-v1.2/` | Carrier board: EasyEDA project |
| `doc/` | Architecture, protocol, hardware and manufacturing notes |
| `third_party/` | Third-party components kept with their original licenses |

## Building

Prerequisites: the official Pico SDK (`PICO_SDK_PATH`), CMake, Ninja,
arm-none-eabi-gcc, and Rust for the desktop app. picotool is required for Debug
Core updates; OpenOCD covers SWD/GDB and Work uploads.

```sh
git submodule update --init --recursive
export PICO_SDK_PATH=/path/to/pico-sdk

./build.sh build            # Debug Core UF2s -> RELEASE/artifacts/
./build.sh test --offline   # host, protocol and board offline checks
cargo test --manifest-path desktop/Cargo.toml --workspace

cargo run --manifest-path desktop/Cargo.toml -p picoharness-desktop
```

`./build.sh flash` upgrades the Debug Core. `./build.sh test --hardware` runs a
Work self-test and overwrites whatever is on the Work flash, so back it up
first. With several probes attached, select one with `PDBG_SERIAL` or from the
device sidebar in the app.

For more, see the [desktop guide](desktop/README.md), the
[Work-side getting started](firmware/harness-target/GETTING_STARTED.md) and the
[Work communication library](firmware/harness-target/README.md).

## License

Most of the project is MIT. The Debug Core firmware is a combined build whose
logic analyzer is derived from
[gusmanb's LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer)
(GPL-3.0-only); the firmware that includes it is distributed under
GPL-3.0-only, while the desktop app and Work communication library are MIT.
Third-party notices are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).