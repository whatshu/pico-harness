[English](../probe-upstream.md) · **简体中文**

# Debug 探针来源

| 项目 | 来源 |
| --- | --- |
| 上游 | [raspberrypi/debugprobe](https://github.com/raspberrypi/debugprobe) |
| Tag | `debugprobe-v2.3.1` |
| Commit | `39e3c8d15ba36cd607a276d7dfe23bde03e88be0` |
| 原始许可证 | MIT，保留于 [`firmware/harness-probe/LICENSE`](../../firmware/harness-probe/LICENSE) |

Debug 固件以导入源码维护，板级定义、USB PH 接口和 Work 桥接由本仓库实现。FreeRTOS 使用顶层 `third_party/FreeRTOS-Kernel` 依赖。

包含采样派生代码的组合固件许可见[采样器来源](probe-logic-analyzer.md)及[第三方声明](../../THIRD_PARTY_NOTICES.md)。
