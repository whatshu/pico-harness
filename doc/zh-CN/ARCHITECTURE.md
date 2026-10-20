[English](../ARCHITECTURE.md) · **简体中文**

# PicoHarness 架构

PicoHarness 由 Debug 探针、Work 目标固件、主机连接库和桌面应用组成。PC 只连接 Debug 的 USB；Debug 通过 SWD 调试 Work，通过 UART/SPI 传输应用数据，并提供电源控制、测量和被动逻辑采样。

本文描述仓库当前实现及接口边界。构建和使用见[入门](getting-started.md)，验证入口见[测试](desktop-testing.md)。

## 1. 系统结构与代码入口

```text
phdcli / Python / Desktop / Claude & Codex MCP
                    │ authenticated TCP JSON-RPC (local or remote)
                    ▼
                   phd ── device ownership / events / tasks
                    ├── CMSIS-DAP + OpenOCD ─ USB IF0 ─ SWD ─ Work
                    ├── PH control ───────── USB IF1/2 ┐
                    └── PH Bulk ──────────── USB IF3 ─┴─ Debug ─ UART/SPI ─ Work
SUMP client ──────────────────────────────── USB IF4/5 ─ passive GPIO capture
```

| 路径 | 职责 |
| --- | --- |
| [`firmware/harness-probe/`](../../firmware/harness-probe/) | Debug 固件：USB、CMSIS-DAP、PH 消息、Work 桥接、采样及电源管理 |
| [`firmware/harness-target/`](../../firmware/harness-target/) | Work 端 `pico_harness` C 库及应用示例 |
| [`firmware/test/`](../../firmware/test/) | 独立测试固件 |
| [`ph-bridge/proto/ph.proto`](../../ph-bridge/proto/ph.proto) | PC ↔ Debug 的共享消息定义 |
| [`ph-bridge/bridge/`](../../ph-bridge/bridge/) | daemon 内部使用的 Rust PH/CDC/Bulk 硬件传输库 |
| [`ph-bridge/phd/`](../../ph-bridge/phd/) · [`phdcli/`](../../ph-bridge/phdcli/) | 后台服务、设备互斥、事件游标、上传任务和控制程序 |
| [`host/mcp/`](../../host/mcp/) | Rust MCP 服务与 Claude/Codex 适配 |
| [`host/phbridge/`](../../host/phbridge/) | 独立 Python 客户端及 `python3 -m phbridge` CLI |
| [`ph-bridge/client/`](../../ph-bridge/client/) | 通过 socket 调用 phd 的 Rust `PhBoardLink` 与 `CmsisDap` 客户端 |
| [`desktop/app/src/`](../../desktop/app/src/) | `ph-app`：egui 界面、后台会话、GDB、录制与解码 |
| [`pcb/pico-dualcore-devboard-v1.2/`](../../pcb/pico-dualcore-devboard-v1.2/) | 可选双 Pico 载板 EasyEDA 工程 |
| [`script/`](../../script/) · [`host/tools/`](../../host/tools/) | 构建辅助、OpenOCD/GDB 入口及检查工具 |

## 2. 硬件与运行模型

### 板型和连线

Debug 固件支持 `pico_dualcore_pico`（RP2040 Pico）和 `pico_dualcore`（RP2350 Pico 2）。板级定义集中在 [`boards/`](../../firmware/harness-probe/boards/)，两者共用以下载板连线；板名和产物名不带硬件修订号。

| 功能 | Debug GPIO | Work GPIO / 外设 |
| --- | --- | --- |
| SWCLK / SWDIO | GP14 / GP15 | Work SWCLK / SWDIO |
| UART1 TX / RX | GP20 / GP21 | UART0 RX GP13 / TX GP12 |
| SPI1 CLK / MOSI / MISO / CS | GP10 / GP11 / GP8 / GP9 | GP18 / GP20 / GP19 / GP21 |
| 电源使能 / 低有效故障输入 | GP0 / GP1 | TPS22919 电源支路 |
| RUN / BOOTSEL 控制 | GP22 / GP26 | 载板断言电路，高电平断言 |
| Work 电压 ADC | GP28 / ADC2 | Work 3.3 V 检测网络 |
| 测量 I²C SDA / SCL / ALERT | GP4 / GP5 / GP2 | INA237 / INA226 |
| USB 状态 LED | GP25 | Debug 板载 LED |

逻辑采样覆盖 Work GP5、GP10–21、GP26–28，共 16 路；GP25 板载 LED 没有载板采样连接。移植到自定义接线时需同步修改板级映射；电源控制和测量依赖相应外部电路。

### 核心分工与电源管理

- **Core 0**：FreeRTOS、TinyUSB、CMSIS-DAP 任务、电源监测、CDC 和 Bulk USB 收发。
- **Core 1**：关闭中断，以轮询驱动 Work UART、PH 消息执行、SPI Bulk 状态机和逻辑采样完成处理。
- 两核使用单生产者/单消费者环形缓冲、任务邮箱和内存屏障交换数据。PH 控制邮箱只有一个任务槽，主机按请求/应答顺序发送。
- Work 默认断电。Core 0 每 5 ms 检查电源使能、无故障和 Work 电压 ≥2.8 V，决定 I/O 就绪状态；切断电源前撤销跨电源域驱动。相关实现为 [`ph_fw_target_control.c`](../../firmware/harness-probe/src/ph_fw_target_control.c) 和 [`ph_fw_target_io.c`](../../firmware/harness-probe/src/ph_fw_target_io.c)。

USB 重置或卸载会清理 PH、Console、Bulk 和 SUMP 会话状态。Debug 自身升级通过 `DebugBootsel` 请求，在响应发送后进入 ROM BOOTSEL。

## 3. PC ↔ Debug：USB 与 PH 协议

### USB 接口

设备 VID/PID 为 `2e8a:000c`，序列号来自 Flash 唯一 ID。实际描述符见 [`ph_fw_usb.c`](../../firmware/harness-probe/src/ph_fw_usb.c)。

| 接口 | 名称 / 协议 | OUT / IN 端点 | 通知端点 |
| --- | --- | --- | --- |
| IF0 | CMSIS-DAP v2，Vendor `FF/00/00` | `04 / 85` | — |
| IF1/2 | PH Terminal，CDC ACM | `02 / 83` | `82` |
| IF3 | PH Terminal Bulk，Vendor `FF/50/01` | `05 / 86` | — |
| IF4/5 | Pico Harness Logic Analyzer，CDC ACM / SUMP | `06 / 87` | `81` |

表中端点地址均为十六进制。USB 为 Full Speed，数据端点最大包长 64 字节；Microsoft OS 2.0 WinUSB 描述符覆盖 IF0 和 IF3。

### 控制消息

PH Terminal 是二进制协议端口，不是文本终端。消息为包 `picoharness.v1` 中的 `PhMessage`：

```text
<varint 消息长度><protobuf PhMessage>
PhMessage = seq + status + oneof body
```

固件用 nanopb，Rust 用 prost，Python 用生成的 protobuf 类型。单帧含长度前缀应放入固件的 512 字节缓冲；请求用非零 `seq`，响应回显 `seq`，异步事件用 `seq=0`。USB PH 消息不另加应用层 CRC。

| 消息组 | 已处理的消息 |
| --- | --- |
| 板级控制 | `GetInfo`、`GetStatus`、`PowerSet`、`ResetPulse`、`BootselSet`、`EnterBootsel`、`DebugBootsel` |
| 测量 / GPIO | `Measure`、`GpioRead`、`GpioDiag` |
| Console | `ConsoleTx`、`ConsoleData`、`TermConfig`、`TermStatus` |
| Bulk 写入 | `BulkTxBegin`、完成事件 `BulkTxDone` |

`ConsoleTx` / `ConsoleData` 每条最多 128 字节。`TermStatus` 报告普通 UART 或 Harness 模式、实际波特率及 FIFO 状态；`TermConfig` 配置 Work UART 波特率，当前实现固定 8N1，CDC line coding 不控制 Work UART。

状态值由 schema 定义：`0 OK`、`1 FRAME`、`2 CMD`、`3 ARG`、`4 BUSY`、`5 CRC`、`6 ROUTE`、`7 NOTREADY`。具体消息的错误映射以 [`ph_terminal.c`](../../firmware/harness-probe/src/ph_terminal.c) 为准。`GetStatus.vbus_mv` 表示 Work ADC 电压；`Measure` 返回分流电压、总线电压、电流和功率，单位依次为 µV、µV、µA、µW。

### Bulk 数据面

```text
PC ── BulkTxBegin(route, length, crc32c) ──► Debug   [CDC]
PC ◄─ 接受 / 拒绝 ──────────────────────── Debug   [CDC]
PC ── 指定长度的原始字节 ────────────────► Debug   [Vendor OUT]
PC ◄─ BulkTxDone(ok, crc_recv) ──────────── Debug   [CDC 事件]
```

IF3 载荷不加帧或 protobuf，事务长度和整体 CRC-32C 界定数据边界。一次只允许一个 Bulk 事务；期间可执行控制请求，再次开始 Bulk 返回忙。

- `DEBUG=0`：接收并校验，长度为 1–4096 字节；用作 Debug 端数据接收校验。
- `WORK_SPI=1`：要求已发现 Harness 模式。Debug 使用 4096 字节环形缓冲，将数据分成最多 2048 字节的 Work SPI 块；每块通过 UART prepare / CRC16 确认，最后校验整笔 CRC-32C。
- **当前数据面仅支持 PC → Debug / Work。** `BulkRxAvail/Begin/Done` 已有 schema，固件没有处理读回，Vendor IN 不发送数据；Work 发出的 SPI 数据公告也不触发读回。

实现见 [`ph_bulk.c`](../../firmware/harness-probe/src/ph_bulk.c)，主机写入见 [`bulk.rs`](../../ph-bridge/bridge/src/bulk.rs)。

## 4. Debug ↔ Work：UART / SPI 协议

### 传输与发现

Work UART0 使用 GP12 TX / GP13 RX，默认 115200 / 8N1；Debug 使用 UART1 与其交叉连接。普通 Work 固件直接收发原始 UART 数据。`pico_harness` 在 UART 空闲时每秒公告 Hello，Debug 识别后改用 Harness 帧；模式发现和波特率事务由 Debug 固件负责。

Work SPI 为从机，模式 0：CLK GP18、MISO GP19、MOSI GP20、CS GP21。UART 承载命令、长度和确认，SPI 只承载原始应用数据。

### UART 帧格式

所有多字节整数为小端序，帧长为 `10 + payload_length`。

| 偏移 | 内容 |
| --- | --- |
| 0–1 | ASCII `PH` |
| 2 | 版本 `1` |
| 3 | 消息 kind |
| 4–5 | 16 位 sequence |
| 6–7 | payload 长度，0–128 |
| 8 起 | payload |
| 最后 2 字节 | CRC16-CCITT，poly `0x1021`、init `0xffff`，覆盖版本至 payload |

请求 sequence 非零，响应回显 sequence 并设置 kind 的 bit 7，主动 Console 消息用 sequence 0。损坏帧搜索下一个 `PH`；Work 解析器使用 138 字节固定缓冲，半帧空闲超时为 `max(4 个 8N1 字符时间, 20 ms)`，Debug 解析器使用 20 ms。

| kind | 请求 / 事件 | 响应 / 内容 |
| --- | --- | --- |
| `01` | Hello | `81`：`01 07 00 08`，版本 1、UART/SPI/波特率能力、最大块 2048 |
| `02` | Console TX，1–128 字节 | `82`：空 ACK，数据送达应用回调 |
| `03` | 波特率 prepare，u32 baud | `83`：在旧速率回显请求值 |
| `04` / `05` | Verify / Commit，空 | `84` / `85`：实际 baud，u32 |
| `06` | Bulk TX prepare，u16 长度 1–2048 | `86`：SPI RX DMA 就绪后回显长度 |
| `08` | Bulk RX prepare，空 | `88`：SPI TX DMA 就绪后的长度，0 表示无数据 |
| `90` | Work 主动短消息 | 1–128 字节 Console 数据 |
| `91` | Work 主动 SPI 数据公告 | u16 长度 |
| `87` | SPI 传输完成确认 | prepare 的 sequence，payload 为数据 CRC16 |

表中 kind 为十六进制。Work 库实现双向 SPI 协议，但 Debug 的 USB 桥接只转发写入方向。准备好的 Work SPI 事务在 2 秒后过期。

### 波特率事务

Work 在旧速率 ACK 发出且至少等待 100 ms 后切换；若发送队列 1 秒内无法排空，则取消 prepare。切换后必须在 `max(1_500_000, ceil(640_000_000 / actual_baud) + 500_000)` µs 内收到 commit，否则恢复旧速率。事务期间应用 `write()` 和新 Bulk prepare 受背压限制。

Debug 收到 prepare ACK 后等待 105 ms、切换 UART、验证实际速率误差不超过 2%，再提交；失败恢复本地旧速率。Work 依靠自身租约回滚，Debug 当前回滚路径不执行恢复验证。普通 UART 模式只修改 Debug 端速率，Work 应用需自行匹配。

协议实现：[`ph_console.c`](../../firmware/harness-probe/src/ph_console.c) 与 [`pico_harness.c`](../../firmware/harness-target/src/pico_harness.c)。

## 5. Work 通信库与固件示例

在官方 `pico_sdk_init()` 之后导入：

```cmake
add_subdirectory(path/to/firmware/harness-target pico_harness)
target_link_libraries(my_app PRIVATE pico_harness)
```

[`pico_harness.h`](../../firmware/harness-target/include/pico_harness.h) 提供以下 API：

| API | 约定 |
| --- | --- |
| `pico_harness_init(callback, context)` | 初始化传输和接收回调 |
| `pico_harness_poll()` | 推进协议、队列、回调和超时；同一核心频繁调用，回调保持短小 |
| `pico_harness_write(data, size)` | 非阻塞、复制已接受的数据；1–128 字节走 UART，129–2048 字节提供给 SPI；返回 false 时稍后重试 |
| `pico_harness_baud()` | 当前实际 UART 波特率 |
| `pico_harness_pwm(pin, hz, duty)` | 在空闲 GPIO 配置 PWM，返回实际频率；占空比 0–1，端点精确 |

回调内的数据指针仅在回调期间有效。库不动态分配、不使用中断或 RTOS；占用 UART0、PIO0 一个状态机、两个 DMA 通道、16 KiB SPI DMA word 缓冲和有界队列。115200 baud 下建议约每 100 µs 调用 `poll()`，更高速率需要更频繁轮询。GP12/13/18/19/20/21 为传输保留脚，GP5 PWM 仅由示例选择；同一 PWM slice 的 A/B 输出共享频率。

| 目录 | 功能 |
| --- | --- |
| `firmware/harness-target/example/harness-console` | Harness Console 回显，包含 UART 和 SPI 消息处理 |
| `firmware/harness-target/example/harness-terminal` | `help/led/gpio/pwm/uart/adc/info/sleep/reboot` REPL |
| `firmware/harness-target/example/harness-signals` | Console 回显、20 ms 周期输出、GP5 10 kHz / 35% PWM；`stream-on/off` 控制连续输出 |
| `firmware/test/blink` | GP25 LED 每 250 ms 翻转，不依赖通信库 |
| `firmware/test/uart-console` | 115200 / 8N1 原始 UART 字节回显 |
| `firmware/test/work-selftest` | 板级自检，复用 Work SPI 引擎 |
| `firmware/test/bandwidth-work` | RAM 中运行的 PIO/DMA SPI 接收校验夹具，GP5 输出 100 kHz 参考信号 |

这些项目均可独立构建，`PICO_BOARD=pico` / `pico2` 按实际 Work 芯片选择。带宽夹具提供 SWD mailbox 计数和校验结果，需配合主机驱动使用。

## 6. 逻辑分析

[`ph_fw_la.c`](../../firmware/harness-probe/src/ph_fw_la.c) 与 [`logic_analyzer.pio`](../../firmware/harness-probe/src/pio/logic_analyzer.pio) 使用 PIO1 和 DMA 被动采样，不重配 UART/SPI 引脚功能。缓冲上限 32,768 个 32 位样本（128 KiB）；立即采样每点 1 个 PIO 周期，边沿触发每点 4 个周期，实际采样率由分频器决定。

上升/下降沿触发保留 `floor(count/4)` 个历史样本，两个链式 DMA 通道循环写入同一缓冲，完成后归一化顺序。触发在历史窗口填满后武装；触发采样至少需要 4 点。引擎记录实际速率、超时、取消、RX stall、运行状态和触发位置。

[`ph_sump.c`](../../firmware/harness-probe/src/ph_sump.c) 在独立 CDC 上提供 SUMP 子集：ID `1ALS`、reset/run、divider/count/flags 寄存器和 stage-0 单路边沿触发。divider 使用 `100 MHz / (divider + 1)` 参考，触发等待固定 5 秒；metadata、多级触发和 flags 的通道组筛选没有实现。输出固定为每样本 4 字节小端数据，低 16 位通道顺序如下：

| SUMP 通道 | Work GPIO | SUMP 通道 | Work GPIO |
| --- | --- | --- | --- |
| 0 | GP28 | 8 | GP15 |
| 1 | GP27 | 9 | GP14 |
| 2 | GP26 | 10 | GP13 |
| 3 | GP5 | 11 | GP12 |
| 4 | GP17 | 12 | GP19 |
| 5 | GP16 | 13 | GP21 |
| 6 | GP11 | 14 | GP18 |
| 7 | GP10 | 15 | GP20 |

SUMP 流不传递引擎的实际速率、触发位置和错误标记；采样不足请求长度时补零，触发超时也可能输出已采集的数据。客户端时间轴取决于请求速率与实际分频的匹配，不能仅凭收到数据判定触发成功。该接口面向 libsigrok `ols` 客户端，桌面端尚未接入 SUMP。

## 7. 主机与桌面端

### 连接层


`phd` 是设备访问的唯一服务入口。`ph-bridge/bridge` 实现 CDC/PH 和 Bulk，`device` 实现 CMSIS-DAP、OpenOCD 与 picotool。桌面端依赖 socket 客户端 `ph-client`（保留 `ph_app_link` 依赖别名）；Python 脚本保留在 `host/`，通过相同 IPC 调用。共享 schema 位于 `ph-bridge/proto/`，PyO3 直接植入方式已移除。

本地与远程均使用带 token 和版本握手的 TCP JSON-RPC。daemon 串行处理同一设备的固件请求，为每个读取者提供独立事件游标；SWD、GDB、烧录共享互斥所有权。后台烧录任务在客户端断线后继续，可由其他客户端查询和取消；daemon 重启会改变 instance，使旧任务与游标失效。MCP 使用类型化工具、结构化错误和短时任务提交，提供 Claude/Codex 配置。

Linux 通过 sysfs 和 serialport 元数据发现端口，macOS/Windows 通过 serialport 的 USB 接口号分类 PH Terminal 与 Logic CDC，避免向 SUMP 端口发送探测字节。libusb 管理 IF0/IF3；固件为 Windows 的这两个 vendor 接口声明 WinUSB，CDC 保持 usbser。原生后台服务分别使用 systemd user、LaunchAgent、Windows SCM。跨平台工作流会构建与测试各平台；真实 USB 与服务管理器仍需对应系统验证。

连接配置、原生服务、token、远程 GDB 通道见 [daemon 文档](../../ph-bridge/README.md)，完整 socket 契约见 [IPC.md](../../ph-bridge/IPC.md)，Agent 配置见 [MCP 文档](../../host/mcp/README.md)。

### 桌面结构与功能边界

`ph-app` 使用 eframe/egui。`model.rs` 保存设备和页面状态，`live.rs` 用后台线程和命令/事件通道执行设备访问，`telemetry.rs` 管理测量、历史和录制，`logic.rs` 负责采样数据解释，`gdb/` 负责本地 GDB/MI 与 daemon 上的 OpenOCD 会话。

| 组件 | 当前实现 |
| --- | --- |
| `PhBoardLink` | 通过 socket IPC 调用 `phd` 提供身份、状态、电源、测量、复位、Debug BOOTSEL、短 Console 和波特率接口 |
| CMSIS-DAP / SWD | daemon 中的独占 SWD 会话，供暂停/恢复、寄存器、内存读写和导出使用 |
| GDB | 管理 OpenOCD 和 GDB/MI，支持匹配 ELF、源码断点/步进及仅启动 GDB server；连接外部 server 时保留其配置 |
| 固件上传 | Work UF2/ELF/BIN 校验后由 OpenOCD 烧录；Debug UF2 由 picotool 更新并校验重连 |
| Console / Logic 界面 | 具备历史、trace、UART/SPI/I²C/PWM 本地解码及 CSV/VCD 导出代码；硬件调用受下述适配层限制 |

**桌面适配层不提供原始 SPI 配置/传输、GPIO 快照和逻辑采集。** 对应方法直接返回接口迁移错误；`work_link.rs` 仍有 Work 帧及 SPI 调用，`live.rs` 仍调用采集方法，因此 GUI 的文件 Bulk 和 Logic 采样不能作为当前已接通的功能使用。PC → Work Bulk 使用 PH CLI / `BulkWriter`，逻辑采样使用独立 SUMP 客户端。

GDB 托管会话期间 daemon 协调设备所有权，桌面 Live 暂停冲突操作。RP2040 托管 OpenOCD 只调试 core0；此前外部会话若留下 core1 暂停，需复位 Work。固件页拖入 ELF 表示烧录；GDB 页使用同次构建的 ELF 提供符号并附加。

桌面本地数据有界：测量历史最多 200,000 点，命令历史每设备 100 条，trace 最多 4000 条且原始/解码数据各限 2 MiB。Live 暂停仅冻结显示，测量和录制继续；JSONL 每条刷新，可离线加载。录制路径为 `$XDG_DATA_HOME/picoharness/recordings`（默认 `~/.local/share/picoharness/recordings`）或 `%LOCALAPPDATA%/picoharness/recordings`，可用 `PDBG_RECORDINGS` 覆盖。

## 8. 构建与分发

- `./build.sh build` / `build-debug` 构建 RP2040 Debug，产物为 `RELEASE/artifacts/pico_dualcore_debug_pico.{uf2,elf}`，附带校验和及构建元数据。
- RP2350 Debug 通过 CMake 显式选择 `PICO_BOARD=pico_dualcore`，产物名为 `pico_dualcore_debug_pico2`；根构建脚本不打包该板型。
- Debug 构建依赖主机 Pico SDK 2.x、ARM GCC、CMake、Ninja；FreeRTOS、CMSIS-DAP、nanopb 在 `third_party/`，TinyUSB 来自 SDK。
- Work 示例默认 Debug 构建，支持显式 Release；调试符号 ELF 必须与已烧录固件来自同次构建。
- Debug 固件包含 GPL-3.0-only 采样派生代码；桌面端、主机库和 Work 库使用各自适用的许可。来源与许可见[探针来源](probe-upstream.md)、[采样器来源](probe-logic-analyzer.md)及[第三方声明](../../THIRD_PARTY_NOTICES.md)。
