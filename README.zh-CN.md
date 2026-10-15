# PicoHarness

<p align="center"><a href="README.md">English</a> · <strong><a href="README.zh-CN.md">简体中文</a></strong></p>

<p align="center">
  <img src="doc/image/front.png" alt="PicoHarness 载板 —— 正面" width="44%">
  <img src="doc/image/back.png" alt="PicoHarness 载板 —— 背面" width="33%">
</p>

PicoHarness 用**一块普通的 Pico 或 Pico 2 开发板**，为 Pico/Pico 2 目标板提供
完整的调试支持。把探针固件烧进一块多余的 Pico，用 SWD 加一对 UART 接到任意
RP2040/RP2350 板卡上，只要一条 USB 连接，就能获得调试 Pico 固件所需的一切——
桌面程序、Python 脚本或 AI 代理可以同样方便地驱动它。

一条 USB 连接就能提供：

- **CMSIS-DAP v2** —— 对目标 RP2040/RP2350 的 SWD 调试：暂停/恢复、寄存器、
  GDB、内存读写与导出。
- **Console** —— 到目标板的 UART 双向通道；配合通信库时大块数据自动改走
  SPI/DMA。
- **电源控制与测量** —— 开关目标电源支路，读取实时的电压、电流与功率（在载板
  或自定义接线上）。
- **逻辑分析** —— 目标运行期间被动采样 GPIO，支持 UART、SPI、I²C、PWM 解码器。
- **固件上传** —— 通过 SWD 烧写目标板，或通过 USB 升级探针自身。

可选载板（`pcb/`）负责安置探针，并增加目标电源开关（TPS22919）和
INA237/INA226 分流测量。在载板上，探针称为 **Debug Core**，被调试的板卡称为
**Work Core**；探针本身完全不依赖载板。

一切通信走 PDBG Link vendor bulk 协议：小端序、CRC 校验帧，承载 UART、SPI、
电源与逻辑分析命令。

## 快速开始

前置依赖：官方 Pico SDK（`PICO_SDK_PATH`）、CMake、Ninja、
arm-none-eabi-gcc，以及用于桌面端的 Rust。探针升级需要 `picotool`；
SWD/GDB 与目标烧写需要 OpenOCD。

```sh
git submodule update --init --recursive
export PICO_SDK_PATH=/path/to/pico-sdk

./build.sh build            # 探针 UF2 -> RELEASE/artifacts/
./build.sh test --offline   # 主机、协议与板级离线检查
cargo test --manifest-path desktop/Cargo.toml --workspace

cargo run --manifest-path desktop/Cargo.toml -p picoharness-desktop
```

`./build.sh flash` 升级探针；`./build.sh test --hardware` 会在目标板上运行自检
并覆盖目标 Flash，请先备份。多探针时用 `PDBG_SERIAL` 或在桌面端设备侧栏选择。

接上载板（或用线把探针的 SWIO/SWCLK 与 UART0 GP12/13 直接连到目标板；引脚映射
见 `doc/probe-firmware.md`），然后按
[doc/getting-started.md](doc/getting-started.md) 走完四步入门。

## 文档

所有文档统一放在 `doc/` 下，根目录 README 是唯一的 README。

| 文档 | 内容 |
| --- | --- |
| [getting-started.md](doc/getting-started.md) | 入门：构建、烧录并从桌面端驱动目标板 |
| [desktop.md](doc/desktop.md) | 桌面端说明（总览、Console、SWD/GDB、逻辑分析、固件） |
| [desktop-testing.md](doc/desktop-testing.md) | UI 性能测试与自动化 GUI 测试 |
| [work-library.md](doc/work-library.md) | `pico_harness` 通信库：在目标固件中嵌入 Console/SPI 传输 |
| [protocol.md](doc/protocol.md) | PDBG Console 线上协议（UART/SPI） |
| [probe-firmware.md](doc/probe-firmware.md) | 探针固件：CMSIS-DAP + PDBG Link、板级引脚映射 |
| [test-firmware.md](doc/test-firmware.md) | 独立测试固件（blink、UART、带宽测试、自检） |
| [pcb.md](doc/pcb.md) | 载板硬件说明 |
| [probe-logic-analyzer.md](doc/probe-logic-analyzer.md) · [probe-upstream.md](doc/probe-upstream.md) | 第三方依赖说明与上游来源 |

## 目录结构

| 路径 | 内容 |
| --- | --- |
| `desktop/` | Rust/egui 桌面程序（总览、Console、SWD/GDB、逻辑分析、固件）与 USB 控制库 |
| `firmware/harness-probe/` | 探针固件：CMSIS-DAP、PDBG Link、UART/SPI、GPIO 采样、电源与测量 |
| `firmware/harness-target/` | 可导入的 CMake 通信库及目标 Pico 示例 |
| `firmware/test/` | 独立测试固件：blink、普通 UART、带宽测试、自检 |
| `host/` · `script/` | Python CLI（`pdbg`）与构建/检查入口 |
| `pcb/pico-dualcore-devboard-v1.2/` | 可选载板：EasyEDA 工程 |
| `doc/` | 统一文档（见上） |
| `third_party/` | 保留原始许可证的第三方组件 |

## 许可

项目代码大部分为 MIT。探针固件为组合构建，其中的逻辑分析器衍生自
[gusmanb 的 LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer)
（仅 GPL-3.0），因此包含它的固件按 GPL-3.0-only 分发；桌面端与目标通信库为
MIT。第三方声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。