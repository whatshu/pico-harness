[English](../getting-started.md) · **简体中文**

# PicoHarness 入门

系统结构、连线和当前功能边界见 [ARCHITECTURE.md](ARCHITECTURE.md)。USB 接 Debug；示例固件烧录到 Work。

## 1. 构建 Debug 与桌面端

需要官方 Pico SDK 2.x、ARM GCC、CMake、Ninja、Rust，以及 Linux 串口和 USB 设备访问权限。Work 烧录和 GDB 需要 OpenOCD；Debug 升级需要 picotool。

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build
cargo run --manifest-path desktop/Cargo.toml -p ph-app
```

根脚本构建 RP2040 Debug，UF2 位于 `RELEASE/artifacts/pico_dualcore_debug_pico.uf2`。首次烧录可按住 Debug BOOTSEL 接入 USB 后复制 UF2；已运行 PH 固件时可用 `./build.sh flash`。

RP2350 Debug 单独构建：

```sh
cmake -S firmware/harness-probe -B BUILD/harness-probe-pico2 -G Ninja \
  -DPICO_BOARD=pico_dualcore
cmake --build BUILD/harness-probe-pico2
```

## 2. 连接和控制

Debug 枚举出 CMSIS-DAP、PH Terminal、PH Terminal Bulk 和 Logic Analyzer。PH Terminal 传输二进制消息，使用 CLI 或客户端访问。Python CLI 需要 `pyserial`、`protobuf`，Bulk 还需要 `pyusb`。

```sh
PYTHONPATH=host python3 -m phbridge device list
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 power on
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 status
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 measure
```

将 tty 替换为实际 PH Terminal 端口。Work 默认断电，先上电再访问；多个探针时控制命令显式选择端口。当前 Bulk CLI 的 USB 数据面自动选设备，进行 Bulk 或 CLI 升级时只连接一个探针。

## 3. 构建和烧录 Work 示例

```sh
cmake -S firmware/harness-target/example/harness-terminal \
  -B BUILD/harness-terminal -G Ninja \
  -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=Debug
cmake --build BUILD/harness-terminal
```

Work 为 Pico 2 时改用 `PICO_BOARD=pico2` 并新建构建目录。打开桌面 **Firmware** 页，选择实际 Work 芯片后导入生成的 `harness_terminal.uf2`。烧录会覆盖 Work Flash。

| 示例目录 | 用途 |
| --- | --- |
| `firmware/test/blink` | 验证烧录和 LED；GP25 不在载板逻辑采样通道上 |
| `firmware/test/uart-console` | 不依赖通信库的原始 UART 回显，115200 / 8N1 |
| `firmware/harness-target/example/harness-console` | Harness 消息回显 |
| `firmware/harness-target/example/harness-terminal` | REPL，发送 `help` 查看命令 |
| `firmware/harness-target/example/harness-signals` | 周期 UART、GP5 PWM 及 `stream-on/off` 连续输出 |

## 4. Console、Bulk 与逻辑采样

```sh
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 console send 'help'
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 console read
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 term 460800
PYTHONPATH=host python3 -m phbridge -p /dev/ttyACM0 bulk /path/to/payload.bin
```

短消息最多 128 字节；发送命令和读取输出是独立操作，命令过程中已收到的输出可能被前一个 CLI 进程消费，交互程序应保持同一个客户端连接。Harness 固件在空闲时公告协议，Bulk 发送前需等待发现；大文件由 Debug 分成不超过 2048 字节的 SPI 块。USB Bulk 当前只支持写入，不能用大文件回显验证读回。普通 UART 固件改速率时需自行匹配 Work 速率。

逻辑采样使用独立 Logic Analyzer CDC 和 SUMP 客户端（如 PulseView 的 `ols` 驱动），通道对应关系见架构文档。烧录 `harness-signals` 后可观察 GP5 的 10 kHz / 35% PWM，以及 GP12/13 UART 和传输期间的 GP18–21 SPI。缓冲最多 32,768 点；2 MHz 对应最长 16.384 ms。SUMP 只提供协议子集，错误状态和实际速率的报告限制见架构文档。

桌面 Logic 硬件采集和文件 Bulk 入口尚未连接到专用接口，使用上述 CLI / SUMP 路径。

## 5. GDB 与自己的项目

在桌面 Firmware 页烧录 Work，再在 GDB 页导入**同次构建**的 Debug ELF。应用启动 OpenOCD 并附加，可使用断点、Continue、Pause 和步进。**GDB server only** 可供外部 IDE 连接，使用页面显示的实际端口；托管会话期间 Live 释放 USB。

将 Console 通信库接入自己的程序时，在 `pico_sdk_init()` 后添加：

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

调用 `pico_harness_init()`，在同一核心持续调用 `pico_harness_poll()`；`pico_harness_write()` 返回 false 时稍后重试。API、资源占用和 UART/SPI 协议统一见 [ARCHITECTURE.md](ARCHITECTURE.md)。普通应用无需嵌入此库即可使用 SWD/GDB 和被动采样。
