[English](../desktop-testing.md) · **简体中文**

# 构建与测试

本页列出可复用的验证入口。硬件接口和限制见 [ARCHITECTURE.md](ARCHITECTURE.md)；测试代码存在不代表当前硬件路径已经接通。

## 离线检查

从仓库根目录运行；分别需要 Python 依赖、主机 C 编译器和 Rust 依赖。

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

这些检查覆盖 PH 编解码、Work 生产代码的 UART/时钟/PWM 桩回归、Rust 连接层、桌面逻辑和性能统计计算。Work 库回归无需 Pico SDK 或设备。

`./build.sh test --offline` 运行 host/MCP 测试并生成 Work 自检固件，因此需要 Pico SDK 和 ARM 工具链。Python 硬件测试仅在 `PHD_HARDWARE_TEST=1` 且显式指定 `PHD_SERIAL` 时执行；脚本的 `--hardware` 会启用这一模式。socket 集成测试使用真实 loopback 监听，daemon 测试还使用 Unix 伪终端，需要允许本地 socket，但不需要 USB 设备。

覆盖范围包括帧分片、认证/版本拒绝、独立订阅者、事件丢失、设备互斥、上传边界、客户端断开后的后台任务取消、GDB 二进制通道、CLI/Python 互操作及 daemon 重启。MCP 测试使用官方客户端和真实 stdio 子进程。[跨平台 CI](../../.github/workflows/host.yml) 构建并测试 Linux/macOS/Windows；真实硬件和原生服务需在对应系统验证。

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

电流采样与 CSV 校验可单独运行。此检查连续读取测量接口 8 秒，然后使用 desktop 的实际后台线程分别以 1、2、5、10 Hz 采样 8 秒，并调用同一套录制与导出代码。它沿用现有串口波特率，不切换 Work 电源、不烧录固件。输出目录使用绝对路径，且每次选择新目录：

```sh
PDBG_PROBE_SERIAL=YOUR_SERIAL PDBG_MEASUREMENT_OUTPUT=/absolute/path/to/new-run \
  cargo test --manifest-path desktop/Cargo.toml -p ph-app --bin ph-app \
  connected_measurement_rates_and_export -- --ignored --nocapture
python3 host/tools/check_measurement_export.py /absolute/path/to/new-run
```

输出包含原始测量、各档接收事件、JSONL 录制、`export.csv` 和校验结果。校验覆盖行数、顺序、单位转换、时间戳、状态以及 CSV 六位小数的舍入误差。连续读取吞吐不等于 ADC 独立转换速率；电流量化步进也不等于绝对准确度，后者需要参考仪表校准。

先在连接探针的机器启动 `phd`，再显式选择序列号。Python 夹具会改变 Work 电源并恢复初始状态，不烧录固件：

```sh
PHD_HARDWARE_TEST=1 PHD_SERIAL=YOUR_SERIAL PYTHONPATH=host \
  python3 -m unittest discover -s host/tests -p test_ph_device.py -v
```

使用 release daemon，运行可重复的 bridge 延迟、完整性和持续负载检查：

```sh
PYTHONPATH=host python3 host/tools/check_bridge.py --serial YOUR_SERIAL \
  --output BUILD/bridge-hardware/run.json --soak-seconds 60
```

默认覆盖顺序/并发控制请求、USB 包边界上的 Debug Bulk CRC、混合负载和反复连接。
持续负载默认使用 8 个控制客户端和 1 个 4 KiB Bulk 发送者；延迟包含 socket 和实体设备往返。

增加 `--swd-rp2040` 可验证 Work 内存只读访问、跨 MEM-AP 页边界的块/单字比对、
目标 FAULT 恢复和独占资源释放。Work 已运行 `harness-terminal` 时，使用
`--console-terminal --console-iterations 300` 比对两个独立订阅者的完整响应，并报告
第三个慢订阅者是否丢失保留历史。`--console-echo` 则需要专用的字节回显夹具。
这些 Work 选项会临时上电并恢复初始电源状态，不烧录固件。
实际结果和恢复测试范围见 [Linux 实体硬件记录](bridge-hardware-validation.md)。

对比 Debug 固件时，先停止 `phd`，再用[原始 USB 核心测量工具](../../host/tools/check_probe_core.py)。
工具记录延迟原始样本与 CRC 结果，可选测量 Work SPI 期间的控制延迟、SUMP 采样回传，结束恢复 Work 电源。
复现命令和夹具限制见[固件性能验证](probe-core-performance.md)。

设置 `PHD_ADDR` / `PHD_TOKEN_FILE` 可验证远程 daemon。桌面硬件测试同样使用 IPC。daemon 内部硬件夹具位于 `ph-bridge/device`；仅在停止 phd 和其他设备使用者后运行：

```sh
cargo test --manifest-path ph-bridge/Cargo.toml -p ph-device --test live \
  -- --ignored --nocapture
```

这些底层夹具会切换 Work 电源，用于传输层开发。Work 已运行匹配的 `harness-signals` 固件且有对应 ELF 时，可验证托管 GDB：

```sh
cargo run --manifest-path desktop/Cargo.toml -p ph-app --bin gdb_smoke -- \
  --flash-fixture BUILD/harness-signals/harness_signals.elf YOUR_PROBE_SERIAL
```

该模式使用现有 Flash 固件并复位、暂停和恢复 Work，不下载新固件。其他 Flash / Debug 更新测试分别受 `PDBG_FLASH_TEST_IMAGE`、`PDBG_FLASH_TARGET`、`PDBG_DEBUG_IMAGE` 和 `PDBG_PROBE_SERIAL` 控制，启用后会编程所选核心。

Logic 的 desktop → phd → SUMP 硬件验收（沿用原波特率，不烧录固件）：

```sh
PDBG_PROBE_SERIAL=YOUR_SERIAL cargo test --manifest-path desktop/Cargo.toml \
  -p ph-app --bin ph-app connected_sump_capture_and_cancel -- --ignored --nocapture
```

覆盖不同采样长度、速率和停止后重采。其他依赖原始 SPI、UART trigger 命令或旧 capture-pretrigger 能力标记的忽略测试仍需各自的 fixture，不能作为这套 RPC 的通用验收入口。

为 `PDBG_LOGIC_TRIGGER_PIN` 指定已连接测试信号的 Work GPIO，可同时验证上升沿/下降沿（需要 `sump2` 固件）。旧 `host/tools/check_logic_presets.py` 仍依赖已移除的 `capture.*` 契约，不是 `logic.*` 的验收入口。

## UART Apply 与原始 trace

`PYTHONPATH=host python3 host/tools/check_console_baud.py --serial <序列号> --require-trace --output /tmp/baud-check.json`
对 9600..921600 的 8 档预设分别做空闲和 help 输出中的切换。每次发送唯一命令并验证新的错误响应，避免旧 prompt 误判；结束恢复初始波特率。原始 trace 检查需要带 `ut1` 标记的新 Debug 固件，并校验 Hello 和 prepare/verify/commit 双向帧的 CRC。脚本不会刷写固件，也不改变 Work 电源。

Transport trace 的 UART 数据来自固件原始收发事件，不再把 ConsoleData/ConsoleTx 的应用载荷当作原始数据。记录和导出包含设备观测时间、实际波特率及累计省略字节数；TX 表示已进入 UART 队列，不能用作引脚边沿时间。USB 背压会省略 trace 并累计计数，优先保证控制通讯。旧固件会显示需要 ut1 的说明。
