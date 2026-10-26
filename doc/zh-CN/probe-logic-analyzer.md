[English](../probe-logic-analyzer.md) · **简体中文**

# 逻辑采样器来源与许可

上游为 [gusmanb/LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer)，作者 Gustavo Hermoso。采用的源文件是 `Firmware/LogicAnalyzer/LogicAnalyzer.pio`，其固件标识版本为 5.2。原始文件 SHA-256：

```text
3a992a2e86612756e42fefb0668acc9b9c4c0538d4f9c77f86fa7f410fcb04aa
```

本仓库的派生实现为：

- [`src/pio/logic_analyzer.pio`](../../firmware/harness-probe/src/pio/logic_analyzer.pio)
- [`src/ph_fw_la.c`](../../firmware/harness-probe/src/ph_fw_la.c)
- [`src/inc/ph_fw_la.h`](../../firmware/harness-probe/src/inc/ph_fw_la.h)

实现采用简单上升/下降沿采样和 PIO/DMA 配置，适配载板引脚、PIO1、有界 DMA 缓冲、轮询完成、触发超时和预触发环形采样。系统行为统一见 [ARCHITECTURE.md](ARCHITECTURE.md)。上游复杂触发、级联、Wi-Fi 和桌面协议未包含在此实现中。

派生文件和包含它们的组合 Debug 固件按 GPL-3.0-only 分发，许可证保留于 [`LICENSE-GPL-3.0`](../../firmware/harness-probe/LICENSE-GPL-3.0)。分发 UF2 时须一并提供对应源码、构建说明和许可声明；其他组件原有 MIT、BSD、Apache 等声明仍适用。

对应源码在本仓库；构建使用 `./build.sh build-debug`，依赖和板型见架构文档。完整组件清单见 [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md)。
