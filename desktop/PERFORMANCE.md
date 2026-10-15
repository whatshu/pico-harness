# UI performance tests

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
