**English** · [简体中文](zh-CN/desktop-testing.md)

# Build and test

This page lists reusable validation entry points. See [ARCHITECTURE.md](ARCHITECTURE.md) for hardware interfaces and limits; the presence of test code does not establish that a hardware path is connected.

## Offline checks

Run from the repository root. The respective commands require Python dependencies, a host C compiler and Rust dependencies.

```sh
PYTHONPATH=host python3 -m unittest discover -s host/tests -p test_ph_proto.py
python3 firmware/harness-target/test/run.py
cargo test --manifest-path host/ph-bridge/Cargo.toml --lib
cargo test --manifest-path desktop/Cargo.toml --workspace
python3 -m unittest discover -s desktop/tests -v
python3 host/tools/check_pinmap.py
```

These cover PH encoding/decoding, production Work code against UART/clock/PWM stubs, the Rust connection layer, desktop logic and performance calculations. Work library regressions need neither the Pico SDK nor hardware.

`./build.sh test --offline` also builds Work self-test artifacts and requires the Pico SDK and ARM toolchain. The script still discovers and runs device tests under `host/tests`; `--offline` / `--hardware` currently do not change device discovery. Use the individual commands above for fully offline checks.

## GUI automation

```sh
cargo test --manifest-path desktop/Cargo.toml -p ph-app --bin ph-app gui_tests
```

[`gui_tests.rs`](../desktop/app/src/gui_tests.rs) uses egui_kittest / AccessKit to construct the real App and drive its controls. Only the USB session's command/event channels are replaced. No display server, device connection or programming is required.

Coverage includes page navigation, Console focus and history, baud submission state, Live pause, Logic parameters and English/Chinese layouts. Controls have stable accessibility names: icons use helpers such as `theme::icon_button`, and inputs use `theme::accessible_label`. Tests use semantic interactions and command assertions, advancing a bounded number of frames after each action rather than waiting for the app to stop repainting.

## Native-window performance

Requires Linux `/proc`, Python 3 and a working X11/Wayland graphical session. Build with the `ui-perf` feature and specify the actual binary:

```sh
cargo build --manifest-path desktop/Cargo.toml -p ph-app --release --features ui-perf
python3 desktop/tools/ui_perf.py --binary desktop/target/release/ph-app \
  --output BUILD/ui-perf/run-1 --repeat 2
```

Each scenario starts a separate process. Defaults are a 1360 × 940 window, 3 s warmup, 10 s observation and 250 ms process sampling. Use a new output directory each time, keep windows visible and avoid interaction or concurrent heavy workloads.

Scenarios cover Live, paused recording, ordinary/flood/HEX Console, SPI trace, 200,000-sample history, dense Logic, cached registers, memory, GDB source and the firmware page. Defaults use bounded synthetic data without USB discovery or programming; `connected-live` uses a real device.

```sh
python3 desktop/tools/ui_perf.py --binary desktop/target/release/ph-app \
  --output BUILD/ui-perf/connected --scenario connected-live --probe-serial YOUR_SERIAL
python3 desktop/tools/ui_perf.py --binary desktop/target/release/ph-app \
  --output BUILD/ui-perf/budget --scenario console-hex --scenario logic-dense \
  --max-update 50 --max-render 50 --max-rss 350
```

Outputs include `summary.md/json`, per-scenario `ui.json`, process `process.csv/log` and recording fixtures. Failed budgets, invalid fixtures, buffer overflows or timeouts produce a nonzero exit status.

CPU 100% means one logical core; steady-state results exclude warmup. RSS/PSS exclude GPU memory and the compositor. Update time includes data injection and ingestion; CPU-side render time excludes vsync waits, while frame intervals include repaint waits. Synthetic traffic measures UI load. Logic scenarios display one capture and do not measure continuous hardware acquisition or SPI/UART throughput. Compare small differences with repeated runs on the same machine and renderer.

## Hardware checks

`host/tests/test_ph_device.py` automatically finds a device and operates its power and control channels. Close other programs using the serial port before running it. The desktop connection layer has explicitly ignored integration tests:

```sh
cargo test --manifest-path desktop/Cargo.toml -p ph_app_link --test live \
  -- --ignored --nocapture
```

These check PH identity, power, measurements and explicit rejection of interfaces that moved off the control link. They switch Work power on and off.

With matching `harness-signals` firmware already on Work and its ELF available, check a managed GDB session:

```sh
cargo run --manifest-path desktop/Cargo.toml -p ph-app --bin gdb_smoke -- \
  --flash-fixture BUILD/harness-signals/harness_signals.elf YOUR_PROBE_SERIAL
```

This mode uses the existing Flash firmware, resetting, halting and resuming Work without downloading new firmware. Other Flash / Debug update tests are gated by `PDBG_FLASH_TEST_IMAGE`, `PDBG_FLASH_TARGET`, `PDBG_DEBUG_IMAGE` and `PDBG_PROBE_SERIAL`; enabling them programs the selected core.

Ignored tests in `live.rs` / `work_link.rs` that require raw SPI or capture methods, and `host/tools/check_logic_presets.py`, depend on currently rejected or absent interfaces. They are not usable hardware acceptance entry points.
