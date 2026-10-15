# GUI automated testing

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

The test driver is [app/src/gui_tests.rs](app/src/gui_tests.rs). `Gui::new`
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

## Adding tests

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
timing is covered by the [explicit hardware tests in the
README](README.md); CPU, memory and render time by the native-window tests in
[PERFORMANCE.md](PERFORMANCE.md). The three kinds of checks complement each
other.