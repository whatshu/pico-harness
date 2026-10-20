[English](../../README.md) · **简体中文**

# PicoHarness

<p align="center">
  <img src="../image/front.png" alt="PicoHarness 载板正面" width="44%">
  <img src="../image/back.png" alt="PicoHarness 载板背面" width="33%">
</p>

PicoHarness 将 Pico 或 Pico 2 作为 USB 探针，调试 RP2040/RP2350 目标板。
可选载板上，探针称为 **Debug Core**，目标板称为 **Work Core**；载板增加目标
电源开关和 INA237/INA226 测量。自定义接线需匹配固件板级定义。

一条 USB 连接提供四类独立接口：

- **CMSIS-DAP v2**：SWD、OpenOCD/GDB、内存访问和目标烧录。
- **PH Terminal**：protobuf 控制消息、测量和短 UART Console 数据。
- **PH Terminal Bulk**：主机向目标写入原始数据，由 Debug 经 SPI/DMA 转发。
- **Logic Analyzer CDC**：通过 SUMP 客户端进行被动 PIO/DMA 采样。

Rust/egui 桌面端包含总览、Console、SWD、GDB、Logic 和固件页面。桌面 SPI/Bulk
及采集硬件路径尚未连接专用接口；Bulk 写入使用 PH CLI，采样使用 SUMP 客户端。
USB Bulk 读回未实现。接口约定和当前实现边界统一见
[ARCHITECTURE.md](ARCHITECTURE.md)。

## 构建与运行

需要 Pico SDK 2.x（`PICO_SDK_PATH`）、ARM GCC、CMake、Ninja 和 Rust。
Work 烧录/调试使用 OpenOCD，Debug 升级使用 picotool。

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
ph-bridge/target/release/phdcli daemon start
cargo run --manifest-path desktop/Cargo.toml -p ph-app
```

根构建脚本将 RP2040 Debug 固件放入 `RELEASE/artifacts/`。Pico 2 Debug 构建、
连线、Work 示例和 CLI 使用见[入门](getting-started.md)，测试命令及设备要求见
[测试说明](desktop-testing.md)。

桌面端和 host 脚本统一通过 socket IPC 访问 `phd`。daemon 持有 USB 和烧录工具进程，提供本地/远程控制、独立 Console 游标和调试/烧录互斥。参见 [daemon 使用说明](../../ph-bridge/README.md) 和 [Claude/Codex MCP 配置](../../host/mcp/README.md)。

## 文档与源码

| 文档 | 内容 |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | 系统结构、引脚、USB/PH 与 Work 协议、固件、库、桌面边界和构建 |
| [getting-started.md](getting-started.md) | 构建、连接、烧录和接口使用 |
| [desktop-testing.md](desktop-testing.md) | 离线、GUI、原生性能和硬件检查 |
| [probe-upstream.md](probe-upstream.md) · [probe-logic-analyzer.md](probe-logic-analyzer.md) | 来源与许可说明 |

源码按 `firmware/`（Debug、Work 库与夹具）、`host/`（Python 脚本与 MCP）、
`desktop/`（GUI）、`ph-bridge/`（Rust daemon、CLI、IPC 客户端、硬件层与协议）、`pcb/`（载板）、
`script/` 和 `third_party/` 组织。

## 许可

项目原创代码通常为 MIT。组合 Debug 固件包含
[gusmanb LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer) 采样派生代码，
按 GPL-3.0-only 分发；桌面端、主机客户端和 Work 库保留各自适用许可。完整声明见
[THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md)。
