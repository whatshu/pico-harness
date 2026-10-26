**English** · [简体中文](zh-CN/probe-logic-analyzer.md)

# Logic sampler provenance and license

The upstream is [gusmanb/LogicAnalyzer](https://github.com/gusmanb/LogicAnalyzer), by Gustavo Hermoso. The source file used is `Firmware/LogicAnalyzer/LogicAnalyzer.pio`, whose firmware identifies itself as version 5.2. Original file SHA-256:

```text
3a992a2e86612756e42fefb0668acc9b9c4c0538d4f9c77f86fa7f410fcb04aa
```

The derived implementation in this repository consists of:

- [`src/pio/logic_analyzer.pio`](../firmware/harness-probe/src/pio/logic_analyzer.pio)
- [`src/ph_fw_la.c`](../firmware/harness-probe/src/ph_fw_la.c)
- [`src/inc/ph_fw_la.h`](../firmware/harness-probe/src/inc/ph_fw_la.h)

It uses simple rising/falling-edge sampling and PIO/DMA configuration, adapted for the carrier pins, PIO1, a bounded DMA buffer, polled completion, trigger timeouts and a pre-trigger ring. System behavior is documented in [ARCHITECTURE.md](ARCHITECTURE.md). Upstream complex triggers, daisy chaining, Wi-Fi and the desktop protocol are not included.

The derived files and the combined Debug firmware containing them are distributed under GPL-3.0-only. The license is retained in [`LICENSE-GPL-3.0`](../firmware/harness-probe/LICENSE-GPL-3.0). Distributing UF2 files requires corresponding source, build instructions and license notices. Existing MIT, BSD, Apache and other notices on individual components remain applicable.

Corresponding source is in this repository. Build with `./build.sh build-debug`; dependencies and board variants are described in the architecture document. See [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) for the complete component list.
