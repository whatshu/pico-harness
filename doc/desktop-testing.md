**English** · [简体中文](zh-CN/desktop-testing.md)

# Build and test

This page lists reusable validation entry points. See [ARCHITECTURE.md](ARCHITECTURE.md) for hardware interfaces and limits; the presence of test code does not establish that a hardware path is connected.

## Offline checks

Run from the repository root. The respective commands require Python dependencies, a host C compiler and Rust dependencies.

```sh
PYTHONPATH=host python3 -m unittest discover -s host/tests -p test_ph_proto.py
PYTHONPATH=host python3 -m unittest discover -s host/tests -p test_probe_transport.py
python3 firmware/harness-target/test/run.py
cargo build --manifest-path ph-bridge/Cargo.toml --bins
PYTHONPATH=host python3 -m unittest discover -s host/tests
cargo test --manifest-path ph-bridge/Cargo.toml --workspace
cargo test --manifest-path host/mcp/Cargo.toml
cargo test --manifest-path desktop/Cargo.toml --workspace
python3 -m unittest discover -s desktop/tests -v
python3 host/tools/check_pinmap.py
```

These cover PH encoding/decoding, production Work code against UART/clock/PWM stubs, the Rust connection layer, desktop logic and performance calculations. Work library regressions need neither the Pico SDK nor hardware.

`./build.sh test --offline` runs the host/MCP checks and builds Work self-test artifacts, so it also requires the Pico SDK and ARM toolchain. Python hardware tests run only with `PHD_HARDWARE_TEST=1` and an explicit `PHD_SERIAL`; the script's `--hardware` option enables that mode. Socket integration tests use real loopback listeners, and daemon tests also use Unix pseudo-terminals. They require local socket permissions but no USB devices.

The host suite covers fragmented frames, authentication/version rejection, independent subscribers, event gaps, device leases, upload bounds, background task cancellation after client disconnect, GDB binary tunneling, CLI/Python interoperability and daemon restart. MCP tests use the official client and real stdio subprocess. [Native CI](../.github/workflows/host.yml) builds/tests Linux, macOS and Windows; hardware/service checks remain platform-specific.

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

Start `phd` on the machine holding the probe, then select its serial explicitly. The Python fixture changes Work power and restores its initial state; it does not program firmware.

```sh
PHD_HARDWARE_TEST=1 PHD_SERIAL=YOUR_SERIAL PYTHONPATH=host \
  python3 -m unittest discover -s host/tests -p test_ph_device.py -v
```

For repeatable bridge latency and integrity checks, use the release daemon and
save the report under `BUILD/`:

```sh
PYTHONPATH=host python3 host/tools/check_bridge.py --serial YOUR_SERIAL \
  --output BUILD/bridge-hardware/run.json --soak-seconds 60
```

This exercises sequential/concurrent control requests, CRC-checked Debug Bulk
payloads at USB packet boundaries, mixed load and connection churn. The soak
uses eight control clients and one 4 KiB Bulk sender by default. Reported call
latencies include socket and physical-device round trips.

Add `--swd-rp2040` for read-only Work memory checks, block/scalar comparisons
across MEM-AP page boundaries, target FAULT recovery and exclusive-lease cleanup.
With `harness-terminal` already installed on Work, `--console-terminal
--console-iterations 300` compares complete replies at two independent
subscribers and reports whether a third slow subscriber lost retained history.
`--console-echo` instead requires a dedicated byte-echo fixture. These Work
options temporarily enable its rail and restore its initial power state. They
do not program firmware. See the [Linux hardware validation record](bridge-hardware-validation.md)
for measured results and the tested recovery scope.

For Debug firmware comparisons, stop `phd` and use the dedicated raw USB
[core benchmark](../host/tools/check_probe_core.py). It records individual
latencies and CRC results, optionally measures Work SPI/control interference
and SUMP capture readout, and restores Work power. Reproduction commands and
fixture limitations are in [firmware performance validation](probe-core-performance.md).

Set `PHD_ADDR` / `PHD_TOKEN_FILE` to test a remote daemon. Desktop hardware checks use the same socket client. Daemon-internal ignored tests live in `ph-bridge/device`; run them only while other device users and phd are stopped:

```sh
cargo test --manifest-path ph-bridge/Cargo.toml -p ph-device --test live \
  -- --ignored --nocapture
```

These low-level fixtures can toggle Work power and are intended for transport development.

With matching `harness-signals` firmware already on Work and its ELF available, check a managed GDB session:

```sh
cargo run --manifest-path desktop/Cargo.toml -p ph-app --bin gdb_smoke -- \
  --flash-fixture BUILD/harness-signals/harness_signals.elf YOUR_PROBE_SERIAL
```

This mode uses the existing Flash firmware, resetting, halting and resuming Work without downloading new firmware. Other Flash / Debug update tests are gated by `PDBG_FLASH_TEST_IMAGE`, `PDBG_FLASH_TARGET`, `PDBG_DEBUG_IMAGE` and `PDBG_PROBE_SERIAL`; enabling them programs the selected core.

Desktop → phd → SUMP hardware acceptance (preserves the current UART baud; no programming):

```sh
PDBG_PROBE_SERIAL=YOUR_SERIAL cargo test --manifest-path desktop/Cargo.toml \
  -p ph-app --bin ph-app connected_sump_capture_and_cancel -- --ignored --nocapture
```

Covers different capture lengths/rates and cancellation followed by another capture. Other ignored tests requiring raw SPI, UART trigger commands or legacy capture-pretrigger tags still need their specific fixtures and are not general acceptance tests for these RPCs.

Set `PDBG_LOGIC_TRIGGER_PIN` to an explicitly driven Work GPIO to also verify rising/falling captures (requires `sump2` firmware). The legacy `host/tools/check_logic_presets.py` still uses the removed `capture.*` contract and is not an acceptance entry point for `logic.*`.
