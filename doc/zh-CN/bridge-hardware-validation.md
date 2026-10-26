[English](../bridge-hardware-validation.md) · **简体中文**

# Linux bridge 实体硬件验证

2026-10-20，Rust `phd` release 构建在实体 RP2040 PicoHarness 上通过了以下负载。
Python 客户端使用带认证的 TCP IPC，USB/CDC 和 CMSIS-DAP 连接由 `phd` 持有。
测试没有烧录固件。

## 测试环境

| 项目 | 配置 |
| --- | --- |
| 主机 | Linux，回环 TCP `127.0.0.1:44841` |
| 探针序列号 | `YOUR_SERIAL` |
| Debug 固件 | `PicoHarness/6;dualcore;ph-terminal;ph-t-bulk;la-sump` |
| USB | Full Speed，12 Mbit/s |
| Work 固件 | 已安装的 `harness-terminal` |
| Console | Harness 模式，报告波特率 115207 |
| SWD | RP2040 core 0，1 MHz，协商的 DAP 包大小为 64 字节 |
| Work 电源 | 初始关闭；每项 Work 测试后恢复关闭 |

## 实测结果

延迟包含 Python socket 调用和实体设备往返。优化前后使用同一夹具；以下是单次测试
结果，不能直接推广到其他主机或固件。

| 操作 | 优化前 | 最终 release |
| --- | ---: | ---: |
| 64 字节 Debug Bulk，20 次中位数 | 5.99 ms | 1.31 ms |
| 4096 字节 Debug Bulk，20 次中位数 | 16.99 ms | 12.50 ms |
| SWD Flash 读取 256 个字，单次 | 341.70 ms | 22.93 ms |

持久化 Bulk 句柄省去了逐次枚举和接口申请；CMSIS-DAP 块传输减少了 SWD 的 USB
往返。最终读取 1024 个字耗时 92.17 ms。控制请求吞吐在不同轮次间有波动，
这里不将其描述为吞吐提升。

| 负载 | 结果 |
| --- | --- |
| 固定控制负载 | 6800 次顺序、并发及混合查询，无失败 |
| 控制延迟 | 顺序查询中位数 0.81 ms、p99 3.78 ms；8 客户端吞吐 989 次/秒 |
| 固定 Debug Bulk 负载 | 1、63、64、65、512、4096 字节共 240 次传输，含并发查询；CRC32C 全部匹配 |
| 连接反复建立 | 100 次独立 socket 连接，无失败 |
| 持续混合负载 | 60.01 秒，8 个查询客户端与 1 个 4 KiB Bulk 发送者；26,951 次查询、3413 次传输，无失败 |
| 混合负载延迟 | 查询 p99 31.54 ms、最大 271.70 ms；Bulk p99 20.32 ms |
| 持续负载期间资源采样 | 15.00 秒内 CPU 为单核的 2.40%，RSS 始终为 9088 KiB，最多 13 个线程 |
| SWD 完整性 | 块读取与单字读取的 Flash 数据一致，含跨 1 KiB、4 KiB 边界的读取 |
| SWD 错误恢复 | 未映射地址读取返回目标 FAULT，随后的 CPUID 读取成功 |
| SWD 所有权 | 拒绝竞争客户端；持有者关闭 socket 后释放独占资源 |
| Console | 300 条 `help`/`info` 命令；两个订阅者收到完全一致的响应，各 146,323 字节 |
| Console 慢订阅者 | 超出保留历史时明确报告 `gap: true`，活跃订阅者未丢失历史 |
| Work SPI Bulk | 128、2048、4096、8192、16,384 字节各 3 次；分块及整体 CRC 均通过 |
| USB 重枚举 | 3 轮软件触发的 BOOTSEL/应用切换；daemon 实例和客户端 socket 保持不变，控制与缓存 Bulk 在 1.49–1.60 秒内恢复 |
| 最终硬件回归 | 重枚举后 `test_ph_device.py` 的 5 项测试全部通过 |

Work SPI 发送未知终端命令，避免改变 GPIO 或固件。重枚举测试先建立缓存 Bulk
连接，再进入 BOOTSEL，确认应用访问明确报错，然后按所选物理端口查到的 USB
总线和地址执行 `picotool reboot`。随后在同一 daemon、同一客户端连接上验证身份
与 Bulk CRC。这覆盖了 USB 消失和重新出现，没有实际拔插线缆。

## 重现主要负载

在仓库根目录运行，使用已安装上述固件的专用夹具、空闲端口及独立状态目录：

```sh
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
export PHD_ADDR=127.0.0.1:44841
export PHD_STATE_DIR="$PWD/BUILD/bridge-hardware/state"
export PYTHONPATH=host
ph-bridge/target/release/phdcli daemon start
python3 host/tools/check_bridge.py --serial YOUR_SERIAL \
  --output BUILD/bridge-hardware/run.json \
  --swd-rp2040 --console-terminal --console-iterations 300 --soak-seconds 60
PHD_HARDWARE_TEST=1 PHD_SERIAL=YOUR_SERIAL \
  python3 -m unittest discover -s host/tests -p test_ph_device.py -v
ph-bridge/target/release/phdcli daemon stop
```

请将序列号替换为目标夹具。[硬件检查工具](../../host/tools/check_bridge.py)会恢复
Work 电源，并在负载失败时保存已完成部分的记录。主要负载不包含单独运行的
Work SPI、进程资源采样和 BOOTSEL 测量。本次原始记录保存在被忽略的
`BUILD/bridge-hardware/` 目录：`baseline.json`、`optimized.json`、`final.json`、
`resources.json`、`work-spi.json`、`recovery.json`。

## 验证范围

本记录覆盖一台 Linux 主机和一块 RP2040 板。持续运行时长为 60 秒，资源采样为
15 秒，不能据此断言长期无泄漏、物理拔插恢复、完整 1 MiB Work Bulk 或高波特率
Console 吞吐已经验证。本轮未测试烧录、macOS/Windows 或 MCP 集成。

软件回归另覆盖了 PH 分片及损坏帧、超时后的连接失效、事件队列上限、大块接收、
DAP 块响应校验和真实 socket/PTY IPC 行为。入口见[构建与测试](desktop-testing.md)。
