[English](../getting-started.md) · **简体中文**

# PicoHarness 入门

系统结构、连线和当前功能边界见 [ARCHITECTURE.md](ARCHITECTURE.md)。USB 接 Debug；示例固件烧录到 Work。

## 1. 构建 Debug 与桌面端

需要官方 Pico SDK 2.x、ARM GCC、CMake、Ninja、Rust，以及 Linux 串口和 USB 设备访问权限。Work 烧录和 GDB 需要 OpenOCD；Debug 升级需要 picotool。

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
ph-bridge/target/release/phdcli daemon start
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

Debug 提供 CMSIS-DAP、PH Terminal、PH Terminal Bulk 和 Logic Analyzer。`phd` 统一持有设备，Python 脚本和桌面端通过 TCP IPC 调用。将 `ph-bridge/target/release` 加入 PATH；Windows 使用 `.exe`。Python 兼容命令的设备访问只需 protobuf，依赖见 `host/requirements.txt`。

```sh
phdcli device list
phdcli --serial SERIAL power on
phdcli --serial SERIAL status
phdcli --serial SERIAL measure
PYTHONPATH=host python3 -m phbridge -p SERIAL status
```

使用发现结果中的序列号。Work 默认断电；控制、Bulk、SWD 和更新始终按同一序列号选择设备。远程使用设置 `PHD_ADDR` 和本地保存的 `PHD_TOKEN_FILE`。原生服务、各平台权限、远程连接和 GDB 通道见 [daemon 使用说明](../../ph-bridge/README.md)；Claude/Codex 配置见 [MCP 文档](../../host/mcp/README.md)。

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
PYTHONPATH=host python3 -m phbridge -p SERIAL console send 'help'
PYTHONPATH=host python3 -m phbridge -p SERIAL console read
PYTHONPATH=host python3 -m phbridge -p SERIAL term 460800
PYTHONPATH=host python3 -m phbridge -p SERIAL bulk /path/to/payload.bin
```

短消息最多 128 字节；daemon 保留 Console 事件，各读取者维护独立游标。保存返回的 cursor 和 instance，并在 gap 为 true 时报告历史数据丢失。Harness 固件在空闲时公告协议，Bulk 发送前需等待发现；大文件由 Debug 分成不超过 2048 字节的 SPI 块。USB Bulk 当前只支持写入，不能用大文件回显验证读回。普通 UART 固件改速率时需自行匹配 Work 速率。

逻辑采样使用独立 Logic Analyzer CDC 和 SUMP 客户端（如 PulseView 的 `ols` 驱动），通道对应关系见架构文档。烧录 `harness-signals` 后可观察 GP5 的 10 kHz / 35% PWM，以及 GP12/13 UART 和传输期间的 GP18–21 SPI。缓冲最多 32,768 点；2 MHz 对应最长 16.384 ms。SUMP 只提供协议子集，错误状态和实际速率的报告限制见架构文档。

桌面 Logic 通过 phd 的专用 SUMP 线程采集；重新构建后需重启旧 phd。边沿触发需 Debug 固件报告 `sump2` 标记。桌面文件 Bulk 入口尚未接通，使用上述 CLI 路径。

## 5. GDB 与自己的项目

在桌面 Firmware 页烧录 Work，再在 GDB 页导入**同次构建**的 Debug ELF。daemon 启动 OpenOCD，桌面端通过认证 socket 通道连接并附加，可使用断点、Continue、Pause 和步进。**GDB server only** 可供外部 IDE 连接，使用页面显示的实际端口；托管会话期间 daemon 协调设备所有权，Live 暂停冲突操作。

将 Console 通信库接入自己的程序时，在 `pico_sdk_init()` 后添加：

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

调用 `pico_harness_init()`，在同一核心持续调用 `pico_harness_poll()`；`pico_harness_write()` 返回 false 时稍后重试。API、资源占用和 UART/SPI 协议统一见 [ARCHITECTURE.md](ARCHITECTURE.md)。普通应用无需嵌入此库即可使用 SWD/GDB 和被动采样。
