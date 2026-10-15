# Desktop testing

The desktop is validated by three complementary kinds of checks: explicit
hardware tests (see the [desktop guide](desktop.md)), the native-window UI
performance tests below, and the egui_kittest automated GUI tests below.

## UI performance tests

The opt-in `ui-perf` build opens the actual native desktop window and renders the
normal pages. The Python runner measures that process through Linux `/proc`; it
needs Python 3 and a working X11/Wayland session, with no extra Python packages.
The normal desktop build contains no performance fixtures.

```sh
cargo build --manifest-path desktop/Cargo.toml -p picoharness-desktop \
  --release --features ui-perf
python3 desktop/tools/ui_perf.py --output BUILD/ui-perf/run-1 --repeat 2
```

Use a new output directory for each run. Each scenario starts a fresh process at
1360 × 940 logical points, with separate window preferences and recordings.
Warmup defaults to 3 s, observation to 10 s, and process sampling to 250 ms.
Keep the windows visible, avoid interaction, and avoid concurrent builds or
other heavy work. Renderer, scaling, CPU and binary hash are recorded.

The default suite covers idle, Live, paused recording, ordinary/flood/hex Console,
raw SPI trace, the 200,000-sample history limit, ordinary/dense logic captures,
cached running registers, memory inspection, cached GDB source and idle firmware
upload. Synthetic cases feed the existing event consumer and bounded buffers;
they perform no USB enumeration, hardware commands or programming.

The selected probe can also supply actual Live measurements. This case performs
discovery and normal Live communication, without flashing either Core:

```sh
python3 desktop/tools/ui_perf.py --output BUILD/ui-perf/connected \
  --scenario connected-live --probe-serial YOUR_SERIAL
```

Run sustained pressure or enforce budgets explicitly:

```sh
python3 desktop/tools/ui_perf.py --output BUILD/ui-perf/soak \
  --scenario console-flood --scenario trace-bulk --warmup 5 --duration 60
python3 desktop/tools/ui_perf.py --output BUILD/ui-perf/budget \
  --scenario console-hex --scenario logic-dense \
  --max-update 50 --max-render 50 --max-rss 350
```

Budgets depend on the machine and renderer; the runner exits nonzero on any
failed budget, invalid/incomplete fixture, buffer overflow or timeout. Repeat
runs on the same machine before treating a small change as a regression.

- `summary.md` and `summary.json`: workloads, environment, results and failures.
- Each scenario's `ui.json`: update/CPU-side render/frame interval percentiles,
  frame rate, fixture setup time, actual viewport scale and buffer sizes.
- `process.csv`: raw CPU time and RSS/PSS samples, including startup and warmup.
- `process.log`, recording and history fixtures: local diagnostic artifacts.

CPU percentages sum all application threads: **100% means one logical CPU**.
Steady CPU is time weighted and excludes warmup. RSS includes shared resident
mappings; PSS apportions them. GPU VRAM, compositor and unrelated processes are
excluded. CPU peaks depend on the sampling interval. A short-run RSS slope can
reflect allocator/font/driver caches and does not prove or disprove a leak.

UI update timing includes synthetic event generation and normal ingestion.
Eframe's previous-frame CPU-side timing also includes rendering, excluding vsync
waiting; frame interval includes repaint waits and is not a 60 FPS promise.
History setup time includes generating and loading its JSONL fixture. Logic
cases display one decoded capture; they do not measure repeated hardware
acquisition or decoding. Synthetic payload rates describe UI load, not physical
UART/SPI throughput.

```sh
python3 -m unittest discover -s desktop/tests -v
cargo test --manifest-path desktop/Cargo.toml --workspace --features ui-perf
```

Measurement tests cover CPU units, unequal intervals, warmup exclusion,
insufficient samples, fixture validation and buffer bounds. Rust checks cover
performance arguments and percentiles, multi-line Console alignment/history,
and pixel-bounded logic geometry that preserves single-sample pulses and exact
transitions at sample zoom. Capture data, decoders and exports keep full precision.

Performance and UI regression results were collected and validated during
development; the criteria below are enforced by the automated checks in
`desktop/tests/`.

## GUI automated testing

PicoHarness tests the UI with
[egui_kittest](https://github.com/emilk/egui/tree/0.31.1/crates/egui_kittest),
the test companion for egui 0.31.1, together with
[kittest](https://github.com/rerun-io/kittest). Tests construct a real `App`,
run `eframe::App::update`, and find and drive the UI through the AccessKit
accessibility tree. No display server, screenshot matching, screen
coordinates or OS-level keyboard/mouse input is involved.

```sh
# GUI tests only; also works on headless Linux
cargo test --manifest-path desktop/Cargo.toml \
  -p picoharness-desktop --bin picoharness-desktop gui_tests

# All default tests
cargo test --manifest-path desktop/Cargo.toml --workspace
```

The test driver is [desktop/app/src/gui_tests.rs](desktop/app/src/gui_tests.rs). `Gui::new`
replaces only the USB session's command and event channels; pages, layout,
focus, event consumption and the commands sent are all production code. The
simulated device is compiled in only for tests: it never appears in the normal
UI and never opens USB or flashes firmware.

Covered today:

- Six-page shortcuts, tab clicks, and navigation that wraps around the ends.
- Console auto-focus, Enter submitting `hi\n`, up/down history with draft
  restore; the search field keeps focus and the input refocuses when returning
  from the history viewer.
- Baud presets, the Apply command, the button disabling while awaiting
  confirmation, and the state after confirmation.
- Overview pause, sustained measurements, and resuming at the latest readings.
- GPn rename, rising/falling-edge icons, virtual decoder channels, and window
  scaling.
- Logic Run / Stop submitting the actual capture duration, sample rate, trigger
  pin and timeout.
- Button heights, same-row alignment, spacing, and window edges across
  English/Chinese, 960 × 700 / 1360 × 940, and expanded/collapsed sidebars.
  Geometry assertions read the accessibility tree; they do not compare
  screenshots.

### Adding tests

Controls need stable, descriptive accessibility names. Text buttons carry
their own names; icons register a name and tooltip via `theme::icon_button` /
`icon_toggle` / `icon_menu`, and text inputs annotate themselves with
`theme::accessible_label`. Renaming a GPn channel does not change its widget
identity.

```rust
let mut gui = Gui::new(Page::Console, [960., 700.], 0);
gui.app.get_by_label("Console input").type_text("hi");
gui.settle();
gui.app.press_key(egui::Key::Enter);
gui.settle();
assert!(matches!(
    gui.commands.try_recv().unwrap(),
    live::Command::DataSend(bytes) if bytes == b"hi\n"
));
```

`click()`, `focus()` and `type_text()` are semantic control actions; shortcuts
use the harness's egui event injection. Call `settle()` after each interaction
to advance three frames. The app keeps requesting repaints, so a loop that
waits for full idleness will hang. Don't let a test degrade into calling a
business method and checking a field, or into clicking by the screen position
of displayed text.

GUI tests verify interactions and command boundaries. Real UART / SPI / PIO
timing is covered by the [explicit hardware tests in the desktop
guide](desktop.md); CPU, memory and render time by the [native-window tests
above](#ui-performance-tests). The three kinds of checks complement each
other.