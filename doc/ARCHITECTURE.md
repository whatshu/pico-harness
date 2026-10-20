**English** · [简体中文](zh-CN/ARCHITECTURE.md)

# PicoHarness architecture

PicoHarness consists of a Debug probe, Work target firmware, host connection libraries and a desktop app. The PC connects only to Debug over USB. Debug controls Work through SWD, carries application data over UART/SPI, and provides power control, measurements and passive logic capture.

This document describes the current implementation and interface limits. See [getting started](getting-started.md) for building and use, and [testing](desktop-testing.md) for validation entry points.

## 1. System structure and code entry points

```text
PC
├─ OpenOCD / CMSIS-DAP client ─ USB IF0 ─── SWD ───────── Work
├─ ph-bridge / phbridge ─────── USB IF1/2 ─ PH messages ┐
├─ BulkWriter / ph bulk ─────── USB IF3 ─── raw data ───┼─ Debug ─ UART/SPI ─ Work
└─ SUMP client (e.g. PulseView) USB IF4/5 ─ PIO/DMA ────┘           passive GPIO capture
```

| Path | Responsibility |
| --- | --- |
| [`firmware/harness-probe/`](../firmware/harness-probe/) | Debug firmware: USB, CMSIS-DAP, PH messages, Work bridging, capture and power management |
| [`firmware/harness-target/`](../firmware/harness-target/) | Work `pico_harness` C library and application examples |
| [`firmware/test/`](../firmware/test/) | Standalone test firmware |
| [`proto/ph.proto`](../proto/ph.proto) | Shared PC ↔ Debug message definitions |
| [`host/ph-bridge/`](../host/ph-bridge/) | Rust `ph_bridge` library, Rust `ph` CLI and optional PyO3 bindings |
| [`host/phbridge/`](../host/phbridge/) | Independent Python client and `python3 -m phbridge` CLI |
| [`desktop/ph_app_link/`](../desktop/ph_app_link/) | `PhBoardLink` control adapter and `CmsisDap` SWD client |
| [`desktop/app/src/`](../desktop/app/src/) | `ph-app`: egui UI, background sessions, GDB, recording and decoding |
| [`pcb/pico-dualcore-devboard-v1.2/`](../pcb/pico-dualcore-devboard-v1.2/) | EasyEDA project for the optional dual-Pico carrier |
| [`script/`](../script/) · [`host/tools/`](../host/tools/) | Build helpers, OpenOCD/GDB entry points and checks |

## 2. Hardware and execution model

### Boards and wiring

Debug firmware supports `pico_dualcore_pico` (RP2040 Pico) and `pico_dualcore` (RP2350 Pico 2). Definitions are centralized in [`boards/`](../firmware/harness-probe/boards/). Both use the carrier wiring below; board and artifact names omit hardware revision suffixes.

| Function | Debug GPIO | Work GPIO / peripheral |
| --- | --- | --- |
| SWCLK / SWDIO | GP14 / GP15 | Work SWCLK / SWDIO |
| UART1 TX / RX | GP20 / GP21 | UART0 RX GP13 / TX GP12 |
| SPI1 CLK / MOSI / MISO / CS | GP10 / GP11 / GP8 / GP9 | GP18 / GP20 / GP19 / GP21 |
| Power enable / active-low fault input | GP0 / GP1 | TPS22919 power branch |
| RUN / BOOTSEL control | GP22 / GP26 | Carrier assertion circuits, active high |
| Work voltage ADC | GP28 / ADC2 | Work 3.3 V sensing network |
| Measurement I²C SDA / SCL / ALERT | GP4 / GP5 / GP2 | INA237 / INA226 |
| USB status LED | GP25 | Debug onboard LED |

Logic capture covers Work GP5, GP10–21 and GP26–28: 16 channels. The GP25 onboard LED has no carrier capture connection. Custom wiring requires matching board mappings; power control and metering require the corresponding external circuits.

### Core responsibilities and power management

- **Core 0** runs FreeRTOS, TinyUSB, the CMSIS-DAP task, power monitoring, and CDC/Bulk USB pumps.
- **Core 1** disables interrupts and polls Work UART, PH message execution, the SPI Bulk state machine and logic capture completion.
- The cores exchange data through single-producer/single-consumer rings, a task mailbox and memory barriers. The PH control mailbox has one job slot; the host sends requests sequentially and waits for each response.
- Work starts powered off. Every 5 ms, Core 0 checks power enable, fault state and Work voltage ≥2.8 V to determine I/O readiness. Cross-domain drive is removed before power is cut. See [`ph_fw_target_control.c`](../firmware/harness-probe/src/ph_fw_target_control.c) and [`ph_fw_target_io.c`](../firmware/harness-probe/src/ph_fw_target_io.c).

USB reset or unmount clears PH, Console, Bulk and SUMP session state. For probe updates, `DebugBootsel` requests entry into ROM BOOTSEL after the response has been sent.

## 3. PC ↔ Debug: USB and PH protocol

### USB interfaces

The device VID/PID is `2e8a:000c`; the serial number comes from the Flash unique ID. Actual descriptors are in [`ph_fw_usb.c`](../firmware/harness-probe/src/ph_fw_usb.c).

| Interface | Name / protocol | OUT / IN endpoints | Notification endpoint |
| --- | --- | --- | --- |
| IF0 | CMSIS-DAP v2, Vendor `FF/00/00` | `04 / 85` | — |
| IF1/2 | PH Terminal, CDC ACM | `02 / 83` | `82` |
| IF3 | PH Terminal Bulk, Vendor `FF/50/01` | `05 / 86` | — |
| IF4/5 | Pico Harness Logic Analyzer, CDC ACM / SUMP | `06 / 87` | `81` |

Endpoint addresses above are hexadecimal. USB runs at Full Speed with 64-byte maximum data packets. The Microsoft OS 2.0 WinUSB descriptor covers IF0.

### Control messages

PH Terminal is a binary protocol port. Messages use `PhMessage` in package `picoharness.v1`:

```text
<varint message length><protobuf PhMessage>
PhMessage = seq + status + oneof body
```

Firmware uses nanopb, Rust uses prost, and Python uses generated protobuf types. A complete frame, including its length prefix, must fit the firmware's 512-byte buffer. Requests use nonzero `seq`, responses echo it, and asynchronous events use `seq=0`. USB PH messages have no additional application CRC.

| Message group | Handled messages |
| --- | --- |
| Board control | `GetInfo`, `GetStatus`, `PowerSet`, `ResetPulse`, `BootselSet`, `EnterBootsel`, `DebugBootsel` |
| Measurement / GPIO | `Measure`, `GpioRead`, `GpioDiag` |
| Console | `ConsoleTx`, `ConsoleData`, `TermConfig`, `TermStatus` |
| Bulk writes | `BulkTxBegin`, completion event `BulkTxDone` |

`ConsoleTx` / `ConsoleData` carry at most 128 bytes each. `TermStatus` reports plain UART or Harness mode, actual baud and FIFO state. `TermConfig` changes Work UART baud; the implementation uses fixed 8N1. CDC line coding does not control Work UART.

The schema defines status values `0 OK`, `1 FRAME`, `2 CMD`, `3 ARG`, `4 BUSY`, `5 CRC`, `6 ROUTE` and `7 NOTREADY`. Per-message error mapping is implemented in [`ph_terminal.c`](../firmware/harness-probe/src/ph_terminal.c). `GetStatus.vbus_mv` is the Work ADC voltage. `Measure` returns shunt voltage, bus voltage, current and power in µV, µV, µA and µW respectively.

### Bulk data plane

```text
PC ── BulkTxBegin(route, length, crc32c) ──► Debug   [CDC]
PC ◄─ accept / reject ───────────────────── Debug   [CDC]
PC ── raw bytes of the declared length ───► Debug   [Vendor OUT]
PC ◄─ BulkTxDone(ok, crc_recv) ───────────── Debug   [CDC event]
```

IF3 payloads contain no framing or protobuf. The transaction length and overall CRC-32C define the data boundary. Only one Bulk transaction may run at a time. Control requests can run during it; another Bulk begin returns busy.

- `DEBUG=0` receives and checks 1–4096 bytes, providing a Debug-side payload integrity check.
- `WORK_SPI=1` requires Harness mode to have been discovered. Debug uses a 4096-byte ring and splits data into Work SPI blocks of at most 2048 bytes. Each block uses UART prepare / CRC16 confirmation, followed by an overall CRC-32C check.
- **The data plane currently supports only PC → Debug / Work.** `BulkRxAvail/Begin/Done` exist in the schema, but firmware does not handle readback and Vendor IN sends no data. Work SPI data announcements do not initiate readback either.

See [`ph_bulk.c`](../firmware/harness-probe/src/ph_bulk.c) and the host writer in [`bulk.rs`](../host/ph-bridge/src/bulk.rs).

## 4. Debug ↔ Work: UART / SPI protocol

### Transport and discovery

Work UART0 uses GP12 TX / GP13 RX, initially 115200 / 8N1, cross-connected to Debug UART1. Plain Work firmware exchanges raw UART bytes. `pico_harness` announces Hello once per second when UART is idle; Debug switches to Harness framing after recognizing it. Debug firmware owns discovery and baud transactions.

Work is an SPI mode-0 slave: CLK GP18, MISO GP19, MOSI GP20 and CS GP21. UART carries commands, lengths and confirmations; SPI carries only raw application data.

### UART frame format

All multibyte integers are little endian. Frame length is `10 + payload_length`.

| Offset | Content |
| --- | --- |
| 0–1 | ASCII `PH` |
| 2 | Version `1` |
| 3 | Message kind |
| 4–5 | 16-bit sequence |
| 6–7 | Payload length, 0–128 |
| 8 onward | Payload |
| Last 2 bytes | CRC16-CCITT, poly `0x1021`, init `0xffff`, covering version through payload |

Requests use nonzero sequences. Responses echo the sequence and set kind bit 7; unsolicited Console messages use sequence 0. Corrupt frames resynchronize at the next `PH`. The Work parser has a fixed 138-byte buffer and expires partial frames after an idle gap of `max(4 character times at 8N1, 20 ms)`. The Debug parser uses 20 ms.

| kind | Request / event | Response / content |
| --- | --- | --- |
| `01` | Hello | `81`: `01 07 00 08`, version 1, UART/SPI/baud capabilities, maximum block 2048 |
| `02` | Console TX, 1–128 bytes | `82`: empty ACK; application callback receives the data |
| `03` | Baud prepare, u32 baud | `83`: requested baud echoed at the old rate |
| `04` / `05` | Verify / Commit, empty | `84` / `85`: actual baud, u32 |
| `06` | Bulk TX prepare, u16 length 1–2048 | `86`: length echoed after SPI RX DMA is armed |
| `08` | Bulk RX prepare, empty | `88`: length after SPI TX DMA is armed; 0 means no data |
| `90` | Work unsolicited short message | 1–128 bytes of Console data |
| `91` | Work unsolicited SPI data announcement | u16 length |
| `87` | SPI transfer completion | Sequence of the prepare request; payload is data CRC16 |

Kinds above are hexadecimal. The Work library implements bidirectional SPI, while the Debug USB bridge forwards only writes. Prepared Work SPI transfers expire after 2 seconds.

### Baud transaction

Work switches after the old-rate ACK has drained and at least 100 ms has elapsed. Prepare is cancelled if the transmit queue cannot drain within 1 second. After switching, commit must arrive within `max(1_500_000, ceil(640_000_000 / actual_baud) + 500_000)` µs, otherwise Work restores its previous baud. Application `write()` and new Bulk prepares are backpressured during the transaction.

Debug waits 105 ms after the prepare ACK, changes its UART, verifies actual baud within 2%, then commits. On failure it restores its local old rate. Work rolls back through its own lease; Debug's rollback path does not verify restoration. In plain UART mode, only Debug's rate changes, so the Work application must match it.

Protocol implementations: [`ph_console.c`](../firmware/harness-probe/src/ph_console.c) and [`pico_harness.c`](../firmware/harness-target/src/pico_harness.c).

## 5. Work library and firmware examples

Import after the official `pico_sdk_init()`:

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

[`pico_harness.h`](../firmware/harness-target/include/pico_harness.h) provides:

| API | Contract |
| --- | --- |
| `pico_harness_init(callback, context)` | Initialize transport and the receive callback |
| `pico_harness_poll()` | Advance protocol, queues, callbacks and timeouts; call frequently on one core and keep callbacks short |
| `pico_harness_write(data, size)` | Nonblocking; copies accepted data. 1–128 bytes use UART; 129–2048 bytes are offered over SPI. Retry later on false |
| `pico_harness_baud()` | Current actual UART baud |
| `pico_harness_pwm(pin, hz, duty)` | Configure PWM on a free GPIO and return actual frequency; duty is 0–1 with exact endpoints |

Callback data pointers are valid only during the callback. The library uses no dynamic allocation, interrupts or RTOS. It occupies UART0, one PIO0 state machine, two DMA channels, 16 KiB of SPI DMA word buffers and bounded queues. At 115200 baud, call `poll()` about every 100 µs; higher rates require more frequent polling. GP12/13/18/19/20/21 are reserved for transport. GP5 PWM is an example choice; A/B outputs on one PWM slice share frequency.

| Directory | Behavior |
| --- | --- |
| `firmware/harness-target/example/harness-console` | Harness Console echo, with UART and SPI message handling |
| `firmware/harness-target/example/harness-terminal` | `help/led/gpio/pwm/uart/adc/info/sleep/reboot` REPL |
| `firmware/harness-target/example/harness-signals` | Console echo, 20 ms periodic output, GP5 10 kHz / 35% PWM; `stream-on/off` controls continuous output |
| `firmware/test/blink` | Toggle the GP25 LED every 250 ms without the library |
| `firmware/test/uart-console` | Plain 115200 / 8N1 UART byte echo |
| `firmware/test/work-selftest` | Board self-test using the shared Work SPI engine |
| `firmware/test/bandwidth-work` | PIO/DMA SPI receive checker running in RAM, with a GP5 100 kHz reference signal |

All are standalone projects. Select `PICO_BOARD=pico` / `pico2` for the actual Work chip. The bandwidth fixture exposes SWD mailbox counters and validation results and requires a host driver.

## 6. Logic capture

[`ph_fw_la.c`](../firmware/harness-probe/src/ph_fw_la.c) and [`logic_analyzer.pio`](../firmware/harness-probe/src/pio/logic_analyzer.pio) use PIO1 and DMA for passive capture without changing UART/SPI pin functions. The buffer holds at most 32,768 32-bit samples (128 KiB). Immediate capture uses 1 PIO cycle per sample; edge capture uses 4. The divider determines actual sample rate.

Rising/falling-edge capture retains `floor(count/4)` history samples. Two chained DMA channels cycle through the same buffer, with sample order normalized on completion. The trigger arms once the history window is filled; triggered capture requires at least 4 samples. The engine records actual rate, timeout, cancellation, RX stall, running state and trigger position.

[`ph_sump.c`](../firmware/harness-probe/src/ph_sump.c) exposes a SUMP subset on a separate CDC: ID `1ALS`, reset/run, divider/count/flags registers and a stage-0 single-channel edge trigger. Divider uses a `100 MHz / (divider + 1)` reference and trigger wait is fixed at 5 seconds. Metadata, multistage triggers and flags-based channel-group filtering are not implemented. Output is always 4 little-endian bytes per sample; the low 16 bits use this channel order:

| SUMP channel | Work GPIO | SUMP channel | Work GPIO |
| --- | --- | --- | --- |
| 0 | GP28 | 8 | GP15 |
| 1 | GP27 | 9 | GP14 |
| 2 | GP26 | 10 | GP13 |
| 3 | GP5 | 11 | GP12 |
| 4 | GP17 | 12 | GP19 |
| 5 | GP16 | 13 | GP21 |
| 6 | GP11 | 14 | GP18 |
| 7 | GP10 | 15 | GP20 |

The SUMP stream omits actual rate, trigger position and error flags. Short captures are padded with zeros, and a trigger timeout can still return captured data. Client timing depends on how closely the requested rate matches the actual divider; receiving data alone does not prove the trigger succeeded. The interface targets libsigrok `ols` clients. The desktop has no SUMP integration.

## 7. Host and desktop

### Connection layer

Rust `ph_bridge::PhClient` handles sequential requests/responses and asynchronous events; `BulkWriter` accesses IF3. prost generates Rust types from the schema during builds. Generated firmware nanopb and Python protobuf files are checked in. Python `host/phbridge` implements its own serial client; the PyO3 `phbridge._native` extension is optional and not required by the CLI.

Rust discovery reads Linux sysfs and groups PH Terminal and Logic CDC ports by serial number. Automatic discovery on macOS/Windows currently returns an empty list. `script/pdbgctl.sh` wraps the Python PH CLI and executes `python3 -m phbridge`.

### Desktop structure and feature limits

`ph-app` uses eframe/egui. `model.rs` holds device/page state; `live.rs` performs device operations on a background thread through command/event channels. `telemetry.rs` manages measurements, history and recording; `logic.rs` interprets capture data; `gdb/` manages GDB/MI and OpenOCD subprocesses.

| Component | Current implementation |
| --- | --- |
| `PhBoardLink` | Identity, status, power, measurement, reset, Debug BOOTSEL, short Console and baud interfaces through `ph_bridge` |
| CMSIS-DAP / SWD | Separate USB client for halt/resume, registers, memory reads/writes and export |
| GDB | Managed OpenOCD and GDB/MI, matching ELF, source breakpoints/stepping and server-only mode; external servers retain their configuration |
| Firmware upload | Work UF2/ELF/BIN validation and OpenOCD programming; Debug UF2 updates through picotool with reconnection checks |
| Console / Logic UI | History, trace, local UART/SPI/I²C/PWM decoders and CSV/VCD export code; hardware calls are subject to the adapter limits below |

**The desktop adapter rejects raw SPI configuration/transfers, GPIO snapshots and logic acquisition.** These methods return interface-migration errors. `work_link.rs` still contains Work framing and SPI calls, and `live.rs` still calls capture methods, so GUI file Bulk and Logic acquisition are not connected features. Use the PH CLI / `BulkWriter` for PC → Work Bulk and a separate SUMP client for capture.

Desktop Live releases USB during managed GDB sessions. Managed RP2040 OpenOCD debugs core0 only; reset Work if a previous external session left core1 halted. Dropping an ELF on the Firmware page means programming; the GDB page uses symbols from the same build to attach.

Desktop data is bounded: measurement history has at most 200,000 samples, command history has 100 entries per device, and trace has at most 4000 records with separate 2 MiB raw/decoded limits. Live pause freezes display only; measurement and recording continue. Each JSONL sample is flushed and recordings can be loaded offline. Recordings live in `$XDG_DATA_HOME/picoharness/recordings` (default `~/.local/share/picoharness/recordings`) or `%LOCALAPPDATA%/picoharness/recordings`, overridden by `PDBG_RECORDINGS`.

## 8. Builds and distribution

- `./build.sh build` / `build-debug` builds RP2040 Debug into `RELEASE/artifacts/pico_dualcore_debug_pico.{uf2,elf}`, with checksums and build metadata.
- Build RP2350 Debug through CMake with `PICO_BOARD=pico_dualcore`; the artifact name is `pico_dualcore_debug_pico2`. The root script does not package this board.
- Debug builds require host Pico SDK 2.x, ARM GCC, CMake and Ninja. FreeRTOS, CMSIS-DAP and nanopb are under `third_party/`; TinyUSB comes from the SDK.
- Work examples default to Debug builds and support explicit Release builds. The symbol ELF must come from the same build as the programmed firmware.
- Debug firmware contains GPL-3.0-only derived sampler code. The desktop, host libraries and Work library retain their applicable licenses. See [probe provenance](probe-upstream.md), [sampler provenance](probe-logic-analyzer.md) and [third-party notices](../THIRD_PARTY_NOTICES.md).
