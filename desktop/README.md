# PicoHarness desktop

```sh
cargo run --manifest-path desktop/Cargo.toml -p picoharness-desktop
```

Requires Rust and USB access to the Debug Core's PDBG/CMSIS-DAP interfaces.
Work firmware upload uses OpenOCD; Debug Core updates use picotool. Neither
programmer needs to be running beforehand. Linux uses X11/XWayland when
available because winit 0.30's native Wayland backend does not deliver file drops.

- **Live**: shared power/voltage/current timeline, pause refresh without stopping
  measurement or recording, resume at the latest sample, JSONL history and CSV export.
- **Console**: text and file sends share the Work console. Short messages use
  UART; compatible Work firmware switches bulk data to SPI/DMA. Up/Down recall
  commands and restore your draft. Coordinated baud changes verify both ends and
  roll back on failure. Transport trace decodes protocol commands and exports
  both their explanations and raw UART/SPI bytes.
- **SWD/GDB**: halt/resume, cached gray registers while running, memory inspection
  and range export as binary, Intel HEX or CSV. GDB needs matching ELF symbols.
- **Logic**: passive GPIO capture and configurable decoder instances (UART, SPI,
  I²C, PWM); VCD, raw CSV and decoded CSV export. Async-capable Debug firmware can
  sample while SPI transfers run. Older firmware supports synchronous capture.
- **Firmware**: upload Work UF2/ELF/BIN, including native file drops; settings
  provide the separate Debug Core UF2 upgrade entry. Chip family and image ranges
  are validated before programming. Debug updates also check the Debug firmware signature
  and target the selected physical USB port, then verify serial-number reconnection.

The device sidebar starts collapsed; no demo data or invented observations are
shown. Plain UART firmware remains supported, but coordinated baud changes and
SPI bulk require the [Work library](../firmware/harness-target/README.md).

Page tabs show `[1]`–`[6]`: **Ctrl+1** Overview, **Ctrl+2** Console, **Ctrl+3** SWD,
**Ctrl+4** GDB, **Ctrl+5** Logic, **Ctrl+6** Firmware. **Ctrl+Tab** advances and
**Ctrl+Shift+Tab** goes back, wrapping at the ends. Entering Console focuses its
command input; **Enter** sends. Repaints leave a manually focused search field
alone. Device and Logic toolbars use consistent 26 px vector buttons with hover
descriptions and accessible names.

Drag a chart in Overview to pan the shared timeline and that channel's value axis.
The mouse wheel or a trackpad pinch zooms around the pointer; horizontal trackpad
scroll pans time. Double-click or choose **Fit charts** to restore the default
view. The charts fill the window height, with room for statistics and health.
Measurements and recording continue while browsing earlier samples.

Use the **pause icon** in Live to freeze the graph and metric values; the play
icon resumes at the latest sample. **Start recording** controls durable recording
independently; resuming refresh jumps to the latest measurements. Console's
**arrow to bottom** toggles automatic scrolling; re-enabling it jumps to the latest output.

In Console, select 9600, 19200, 38400, 57600, 115200, 230400, 460800 or 921600 baud
and choose **Apply**. Work acknowledges at the old
rate, switches, verifies, then commits; failure verifies a rollback. The initial
short send listens for protocol identification for up to 1.2 seconds before a
plain-UART fallback, without injecting probe bytes. Timeouts account for the
configured baud; low-rate messages can take several seconds. Reconnecting keeps
the actual baud. Up/Down works only in the command input, restoring the unsent
draft at the end; there is no Console History button. History is per device,
bounded to 100 commands, and lasts for the current app session.
Long lines scroll horizontally; explicit line breaks retain aligned timestamp,
direction and message columns. Only visible Console rows are formatted and drawn.

**Transport trace** shows command names, sequence numbers, payload sizes, baud
parameters, capabilities and CRC confirmation alongside the raw UART frames.
SPI rows show bulk payload sizes; their control/CRC acknowledgments travel on
UART. Split UART frames are assembled separately for TX and RX. The read-only
buffer retains up to 4000 records, with separate 2 MiB limits for raw bytes and
decoded text. CSV exports all retained bytes and explanations. On-screen previews
use the latest 500 records and 64 bytes each, rendering only visible rows.

In SWD **Memory**, addresses accept hexadecimal with or without `0x` / `0X`.
**Read length** controls inspection in bytes; Refresh and Enter honor that length
and limit SRAM reads to the selected chip's RAM end. At RP2040 `0x20041FD0`,
the default 256-byte view reads the remaining 48 bytes, avoiding a SWD FAULT.
One-byte and unaligned inspections display exactly the requested valid bytes.
**Export length** independently controls file export.

For memory export, connect SWD, halt Work, then open **Memory**. Enter the start
address and byte length (1 byte to 4 MiB), select Binary/Intel HEX/CSV, and choose
**Export range**. Running register values are gray snapshots from the last halt.

Logic starts with numerically sorted GPn channels and no assumed signal names.
Click a channel label to rename it, choose its trigger or add a decoder. Decoders
are virtual channels; click their labels to configure protocol parameters,
rename, enable or remove them. Channels offers All, None and PicoHarness defaults;
defaults select UART/SPI/PWM inputs and create matching decoders. Changing decoder
inputs selects the necessary channels automatically. The catalog contains UART,
SPI, I²C and PWM; external sigrok/Python plugins are not loaded.

Choose duration and a tested preset: 100 kHz, 500 kHz, 1/2/5/10/20 MHz. Captures
contain at most 32,768 samples; duration is limited by this buffer, and the returned
actual PIO rate drives the timeline. Rising/falling-edge triggers are visible in
the toolbar and channel label. Timeout and cancellation leave subsequent captures
usable. Sampling does not repurpose active UART/SPI pins.

With `capture-pretrigger` Debug firmware, edge captures retain a quarter-window
of history before the `T` marker, including a UART start bit preceding a rising
edge. The other three quarters follow the trigger. Allow the history interval
to prime before sending; at 500 kHz / 32,768 samples this takes 16.384 ms. Upgrade
Debug Core in Settings for this behavior. Existing firmware remains supported
but captures only after the edge. Channel menus use visibility, trigger and
decoder icons; hover reveals each action and the selected trigger is highlighted.

All GPn labels refer to **Work Core**, not Debug Core. The dualcore carrier exposes
GP5, GP10–21 and GP26–28 to capture. Work GP25 (the Pico onboard LED) is not wired
to these inputs, so the plain `blink` example does not produce a visible waveform.
For Console traffic, GP13 observes host-to-Work bytes and GP12 observes Work output.
Start capture before sending; GP12 needs output or an echo from Work. A silent
firmware still allows GP13 sampling of plain UART bytes without the Harness protocol.

GDB starts a new managed OpenOCD by default. Drop a matching Debug ELF outside the
Firmware page to attach and follow source; in Firmware, ELF drops still program
Work. Follow PC can be re-enabled after browsing another file. Source gutter
breakpoints can be changed while running: interrupt, update, then resume. GDB
selects software or hardware breakpoints from the memory map. **GDB server only**
starts OpenOCD without an ELF or built-in GDB client; the page shows the actual
endpoint for external IDEs. Disconnect shuts down the owned server. USB Live is
released during a managed debug session.

In GDB **Memory**, edit Address then use Read, Enter or the toolbar Refresh.
Write accepts HEX bytes and refreshes the selected address after completion.
Reads beginning in SRAM are limited to the selected chip's RAM end, with the
actual byte count shown. For RP2040, `0x20041FD0` has 48 bytes remaining; a
64-byte request reads those 48 bytes rather than crossing `0x20042000`.

Managed RP2040 sessions debug core0 only. The stock OpenOCD script isolates two
cores; resetting both and resuming only core0 leaves core1 halted, freezing the
shared timer and UART initialization. If a previous external dual-core session
left core1 halted, disconnect it and use the top-level Work Reset before attaching.
An existing external server retains its own target configuration.

Console's Hardware, UART and SPI status icons distinguish responding hardware,
verified Work protocol and pending SPI bulk data; hover for the full details. PWM belongs to Logic. See the
[example walkthrough](../firmware/harness-target/GETTING_STARTED.md).

Closing or reconnecting stops bulk/memory operations between chunks and marks
unfinished transfers cancelled. A baud transaction completes its verified
rollback before releasing USB ownership.

Recordings live under `$XDG_DATA_HOME/picoharness/recordings` (default
`~/.local/share/picoharness/recordings`) or `%LOCALAPPDATA%/picoharness/recordings`.
`PDBG_RECORDINGS` overrides this directory. Each sample is flushed to JSONL;
opening a recording reconstructs the timeline without a connected device.

```sh
cargo test --manifest-path desktop/Cargo.toml --workspace
cargo build --manifest-path desktop/Cargo.toml -p picoharness-desktop
```

Native CPU/memory and frame-time tests, including saturated Console, full history
and dense logic captures, are documented in [PERFORMANCE.md](PERFORMANCE.md).
Headless tests of real page controls, focus, shortcuts and alignment use
egui_kittest / AccessKit; see [GUI_TESTING.md](GUI_TESTING.md).

Hardware tests are opt-in. Work flash tests require `PDBG_FLASH_TEST_IMAGE`,
`PDBG_PROBE_SERIAL`, and `PDBG_FLASH_TARGET`; Debug updates require
`PDBG_DEBUG_IMAGE` and `PDBG_PROBE_SERIAL`. These tests program the selected Core.
The Console/capture test needs the `harness-signals` example running on Work.
The quiet/reconnect test needs `harness-console`; the stream/cancel test needs
`harness-signals`. Back up Work Flash before programming these fixtures.

```sh
PDBG_PROBE_SERIAL=YOUR_SERIAL cargo test --manifest-path desktop/Cargo.toml \
  -p picoharness-desktop --bin picoharness-desktop \
  work_link::tests::connected_work_console_bulk_and_baud -- --ignored --nocapture
```

With a silent Work firmware such as `blink`, this non-programming check verifies
plain UART sends and both trigger edges on Work GP13:

```sh
PDBG_PROBE_SERIAL=YOUR_SERIAL cargo test --manifest-path desktop/Cargo.toml \
  -p picoharness-desktop --bin picoharness-desktop \
  live::tests::connected_plain_uart_console_trigger -- --ignored --nocapture
```

The test asserts all three `hi\n` bytes at 500 kHz and 1/2/5/10/20 MHz on both
edges, twice, plus an immediate capture. It needs `capture-pretrigger` firmware.
With the same silent Work fixture, `live::tests::connected_pretrigger_spi_and_recovery`
also checks SPI CS triggering, UART/SPI decoding in one capture, wrapped-ring
timeout/cancellation and subsequent immediate captures. Neither test flashes Work.
