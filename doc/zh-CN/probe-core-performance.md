[English](../probe-core-performance.md) · **简体中文**

# Debug 固件性能验证

2026-10-21，在 RP2040 探针 `E661AC8863787926` 上，将优化固件与提交 `56aaa45` 的基线比较。
两版使用同一 Linux 主机、USB 连接、现有 Work `harness-terminal` 和 `RelWithDebInfo` 构建。
测量工具直接占用 CDC 与 vendor USB，测量期间停止 `phd`。未烧录 Work 固件；Work 起始断电，结束恢复断电。

## 实测结果

主要对照每种传输/采样各执行 20 次，空闲控制查询执行 100 次。
每笔 16 KiB Work SPI 期间连续发送控制查询：基线共 19,384 次，优化版 22,503 次。
延迟包括主机与设备往返；采样耗时包括请求 1 MHz、无触发的采集，以及完整 128 KiB 回传。

| 指标 | 基线 | 优化版 |
| --- | ---: | ---: |
| Debug Bulk 4096 B，中位 | 12.692 ms | 9.862 ms |
| Debug Bulk 64 B，中位 | 1.216 ms | 1.227 ms |
| 空闲 GetInfo，中位 | 0.661 ms | 0.660 ms |
| Work SPI 期间控制查询，中位 | 0.703 ms | 0.748 ms |
| Work SPI 期间控制查询，p99 | 2.589 ms | 1.200 ms |
| Work SPI 期间控制查询，最大 | 20.870 ms | 4.314 ms |
| Work SPI 16 KiB，中位 | 889.172 ms | 889.815 ms |
| SUMP 32,768 点，成功返回的中位耗时 | 671.938 ms | 326.365 ms |
| 1 秒内完整返回的 SUMP 采样 | 13/20 | 20/20 |

4 KiB Bulk 延迟降低 22.3%，采样与回传耗时降低 51.4%，SPI 期间观察到的最大控制延迟降低 79.3%。
这些是本轮样本统计，不构成延迟上限保证。Work SPI 总吞吐、小 Bulk 和空闲控制延迟没有改善结论。
成功 Bulk 的整笔 CRC-32C 均与主机匹配，Work 路径还逐块校验 Work 返回的 CRC16。

此前在同一夹具上的 A/B/A 对照也出现相同收益：4 KiB Bulk 中位 12.28→9.88→12.63 ms，
成功采样中位 671→289→672 ms。此前两轮基线分别丢失 1/10、3/20 次采样，首次优化版为 15/15 完整返回。
因此主表保留最终一轮自己的耗时和成功次数，没有混用不同轮次的最佳值。

## 代码变化与正确性检查

- Bulk 复用已有查表 CRC，增加增量更新接口；环形缓冲 CRC 和复制按最多两个连续区段处理。
- SPI DMA 启动后返回 Core 1 主循环，由后续 poll 完成，让 UART、PH 控制和采样完成处理继续运行。
  UART CRC 先于 DMA 完成轮询到达时暂存，避免丢回复。
- SUMP worker 独占采样完成轮询，删除外层重复 poll，避免完成事件被消费却未发布给 SUMP。
- SUMP 每次最多批量写 64 字节，只为最后短包显式 flush；USB 背压下保留每条身份回复，避免等自己的 USB 完成回调。
- Bulk 重置先停止 USB 生产者，再由 Core 1 取消 DMA、清理索引，之后才允许复用缓冲和启动新事务。

生产 C 代码在 RP2040/RP2350 配置下共通过 22 项主机模型回归：DMA 等待、移位寄存器排空、重叠传输拒绝、
超时/失电取消、提前到达的 UART CRC、跨环形边界的多块 SPI、重置所有权、CRC 拒绝，以及 SUMP 背压、通道顺序与补零。
两种固件均构建成功；RP2350 仅验证构建和模型，未做实机测量。

额外实机回归完成 70 笔包/环形边界 Debug Bulk，正确拒绝故意错误的 CRC 并恢复；
18 笔 Work SPI 覆盖 128/2048/2049/4096/5000/16384 字节；15 次采样覆盖 4/20/64/1028/32768 点，全部完整返回。
观察到 CS 有效后中断三笔 SPI，均显式失败，Debug Bulk 仍可用，Work 在准备事务租约过期后恢复。

release `phd` 的 5 项 `test_ph_device.py` 实机测试全部通过，另完成 270 次顺序/并发/混合控制查询、
60 笔 Debug Bulk 与 10 次连接。SWD 只读块/标量对照和目标 FAULT 恢复通过；两个 Console 订阅者收到六条命令的相同回复。
兼容性检查使用独立的测试 daemon，结束已关闭。

短暂关电时 Work 仍约为 3.3 V，旧接收事务可能继续保持最多 2 秒；1.1 秒后重试曾失败。
等待 2.2 秒后连续 10 次中断/恢复均成功。保持断电 200 ms 后 Work 降至 30 mV，再上电等待 1.1 秒也能恢复。
测量工具遵循这一既有协议限制，不保证极短关电后立即恢复。

优化 ELF 的 `size` text 增加 1568 字节，BSS 增加 16 字节，新路径没有动态分配。
USB 重置期间的取消由主机模型覆盖；DMA 期间实际拔线/USB 重置、RP2350 实机及长时间稳定性尚未验证。

## 复现

使用专用夹具，运行原始 USB 工具前停止 `phd`，显式指定序列号。
各版本分别构建，切换源码前保存基线镜像。RP2040 命令如下：

```sh
cmake -S firmware/harness-probe -B BUILD/probe-core/current -G Ninja \
  -DPICO_BOARD=pico_dualcore_pico
cmake --build BUILD/probe-core/current -j 4
PYTHONPATH=host python3 -m unittest discover -s host/tests -p test_probe_transport.py -v
PYTHONPATH=host python3 host/tools/check_probe_core.py \
  --serial YOUR_SERIAL --label REVISION --iterations 20 --work-spi --la \
  --output BUILD/probe-core/REVISION.json
PYTHONPATH=host python3 host/tools/check_probe_core.py \
  --serial YOUR_SERIAL --iterations 1 --work-spi --la --regression \
  --output BUILD/probe-core/regression.json
```

每轮测量前烧录对应 Debug 镜像；测量工具本身不烧录。
Work SPI 要求已安装 `harness-terminal`，负载发送未知 Console 命令。
不需要对应夹具操作时省略 `--work-spi` 或 `--la`。
JSON 保存原始延迟、采样失败和电源恢复状态；工作负载失败时退出码非零，仍保存已完成部分。

本地原始记录保存在忽略的 `BUILD/probe-perf/`：`baseline-final.json`、`optimized-final.json`、
`regression-confirmed.json`、`comparison.json`、`phd-compatibility.json`、`phd-device-tests.log`。
原始完整 Flash 备份为 `original.uf2`。对照镜像 SHA-256：

```text
baseline  5e46ad41a7a2709ea2c36b4b6af0f48fb7a5506f39a83124e24f9296472201bb
optimized 98ea6862e4df99ea0f9ad0c0892723bc36fd88a6cb8f6c3073fcc3f084c5f2cf
```
