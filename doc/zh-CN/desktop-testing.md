[English](../desktop-testing.md) · **简体中文**

# 构建与测试

本页列出可复用的验证入口。硬件接口和限制见 [ARCHITECTURE.md](ARCHITECTURE.md)；测试代码存在不代表当前硬件路径已经接通。

## 离线检查

从仓库根目录运行；分别需要 Python 依赖、主机 C 编译器和 Rust 依赖。

```sh
PYTHONPATH=host python3 -m unittest discover -s host/tests -p test_ph_proto.py
python3 firmware/harness-target/test/run.py
cargo test --manifest-path host/ph-bridge/Cargo.toml --lib
cargo test --manifest-path desktop/Cargo.toml --workspace
python3 -m unittest discover -s desktop/tests -v
python3 host/tools/check_pinmap.py
```

这些检查覆盖 PH 编解码、Work 生产代码的 UART/时钟/PWM 桩回归、Rust 连接层、桌面逻辑和性能统计计算。Work 库回归无需 Pico SDK 或设备。

`./build.sh test --offline` 还会构建 Work 自检产物，需要 Pico SDK 和 ARM 工具链。该脚本当前仍发现并运行 `host/tests` 中的设备测试；`--offline` / `--hardware` 没有改变设备探测行为。需要完全离线时使用上面的独立命令。

## GUI 自动化

```sh
cargo test --manifest-path desktop/Cargo.toml -p ph-app --bin ph-app gui_tests
```

[`gui_tests.rs`](../../desktop/app/src/gui_tests.rs) 使用 egui_kittest / AccessKit 构建真实 App 并驱动控件。仅替换 USB 会话的命令/事件通道，不需要显示服务器，也不连接或烧录设备。

覆盖页面导航、Console 焦点和历史、波特率提交状态、Live 暂停、Logic 参数及中英文布局。控件使用稳定的无障碍名称，图标通过 `theme::icon_button` 等注册，输入框通过 `theme::accessible_label` 标记。测试通过语义控件操作和命令断言验证行为，交互后推进有限帧，不等待应用停止重绘。

## 原生窗口性能

需要 Linux `/proc`、Python 3 和可用的 X11/Wayland 图形会话。用 `ui-perf` feature 构建并显式指定实际二进制：

```sh
cargo build --manifest-path desktop/Cargo.toml -p ph-app --release --features ui-perf
python3 desktop/tools/ui_perf.py --binary desktop/target/release/ph-app \
  --output BUILD/ui-perf/run-1 --repeat 2
```

每个场景启动独立进程，默认窗口 1360 × 940、预热 3 秒、观察 10 秒、进程采样间隔 250 ms。每次使用新输出目录，保持窗口可见，避免操作窗口及并行重负载。

场景涵盖 Live、暂停录制、普通/饱和/HEX Console、SPI trace、200,000 点历史、稠密 Logic、缓存寄存器、内存、GDB 源码和固件页。默认使用有界合成数据，不枚举 USB 或烧录；`connected-live` 使用真实设备。

```sh
python3 desktop/tools/ui_perf.py --binary desktop/target/release/ph-app \
  --output BUILD/ui-perf/connected --scenario connected-live --probe-serial YOUR_SERIAL
python3 desktop/tools/ui_perf.py --binary desktop/target/release/ph-app \
  --output BUILD/ui-perf/budget --scenario console-hex --scenario logic-dense \
  --max-update 50 --max-render 50 --max-rss 350
```

输出 `summary.md/json`、场景 `ui.json`、进程 `process.csv/log` 及录制夹具。预算失败、夹具无效、缓冲溢出或超时返回非零。

指标含义：CPU 100% 代表一个逻辑核，稳态结果排除预热；RSS/PSS 不含 GPU 显存和合成器。更新时长包含数据注入和消费，CPU 侧渲染时间不含 vsync 等待，帧间隔包含重绘等待。合成流量衡量 UI 负载；Logic 场景展示单次采样，不衡量连续硬件采集或 SPI/UART 吞吐。小差异应在同机同渲染器重复比较。

## 硬件检查

`host/tests/test_ph_device.py` 自动寻找设备，会操作电源及控制通道；运行前关闭其他占用串口的程序。桌面连接层有显式忽略的集成测试：

```sh
cargo test --manifest-path desktop/Cargo.toml -p ph_app_link --test live \
  -- --ignored --nocapture
```

该组检查 PH 身份、电源、测量和已迁移接口的明确拒绝行为，会开关 Work 电源。

已有匹配 `harness-signals` ELF 和 Work 固件时，可验证托管 GDB 会话：

```sh
cargo run --manifest-path desktop/Cargo.toml -p ph-app --bin gdb_smoke -- \
  --flash-fixture BUILD/harness-signals/harness_signals.elf YOUR_PROBE_SERIAL
```

该模式使用现有 Flash 固件并复位、暂停和恢复 Work，不下载新固件。其他 Flash / Debug 更新测试分别受 `PDBG_FLASH_TEST_IMAGE`、`PDBG_FLASH_TARGET`、`PDBG_DEBUG_IMAGE` 和 `PDBG_PROBE_SERIAL` 控制，启用后会编程所选核心。

`live.rs` / `work_link.rs` 中依赖原始 SPI 和采集方法的忽略测试，以及 `host/tools/check_logic_presets.py`，依赖当前已拒绝或不存在的接口，不作为可运行的硬件验收入口。
