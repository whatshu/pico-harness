**English** · [简体中文](zh-CN/probe-upstream.md)

# Debug probe provenance

| Item | Source |
| --- | --- |
| Upstream | [raspberrypi/debugprobe](https://github.com/raspberrypi/debugprobe) |
| Tag | `debugprobe-v2.3.1` |
| Commit | `39e3c8d15ba36cd607a276d7dfe23bde03e88be0` |
| Original license | MIT, retained in [`firmware/harness-probe/LICENSE`](../firmware/harness-probe/LICENSE) |

Debug firmware is maintained as imported source. This repository implements the board definitions, USB PH interfaces and Work bridge. FreeRTOS uses the top-level `third_party/FreeRTOS-Kernel` dependency.

For the combined firmware containing the derived sampler, see [sampler provenance](probe-logic-analyzer.md) and [third-party notices](../THIRD_PARTY_NOTICES.md).
