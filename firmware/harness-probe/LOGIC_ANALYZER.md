# LogicAnalyzer source attribution

Source: https://github.com/gusmanb/logicanalyzer
Author: gusmanb (Gustavo Hermoso).
Retrieved 2026-09-21 from `master/Firmware/LogicAnalyzer/LogicAnalyzer.pio`
(the firmware at that path identifies itself as version 5.2).
The upstream file's SHA-256 is recorded below so the adapter is tied to exact
source content rather than a moving branch.

`src/pio/logic_analyzer.pio` and `src/logic_analyzer.c` adapt the simple positive /
negative capture post-trigger loop and PIO/DMA configuration. Changes on
2026-09-21: carrier pin mapping, exclusive PIO1 allocation, finite DMA buffer,
polled completion, bounded trigger wait, joined RX FIFO, selectable input
synchronizer bypass, and an immediate one-instruction burst sampler. The
upstream complex trigger, daisy chain, Wi-Fi, and desktop wire protocol are not
included. PDBG remains the USB transport.

On 2026-10-10 the edge sampler gained a pre-trigger DMA ring. Previously sampling
began only after the selected edge, losing the UART start bit on a rising trigger.
The adapter now retains `floor(count / 4)` history samples and then records the
remaining post-trigger samples. Every edge-detector path takes four PIO cycles
per sample; immediate capture still takes one. The returned actual divider rate
defines the timeline. No GPIO function or UART/SPI peripheral is reconfigured.

Two chained DMA channels reuse the existing 128 KiB capture buffer: the data
channel fills it and a one-word control channel rewinds/restarts that channel.
No buffer copy or CPU sampling interrupt is needed. Core 1 polls the PIO completion
flag and freezes/drains DMA before normalizing the ring for USB reads. Timeout
and cancellation stop both channels. Triggered captures require at least four
samples. The trigger arms after the history window is primed, so allow that
initial `history / sample_hz` interval before sending a test signal.

`capture-pretrigger` advertises this behavior. Flags bit 5 marks a ring capture;
bits 16–31 report the first guaranteed post-trigger sample index. The desktop
uses that index for its `T` marker.

The upstream license is GPL-3.0 (see LICENSE-GPL-3.0). These adapted source files and
the combined dualcore firmware containing them are distributed under GPL-3.0-only.
Existing MIT, BSD and Apache notices on other components remain applicable.
Corresponding source and build instructions are in this repository. FreeRTOS
and CMSIS-DAP are checked in under `third_party/`; the Pico SDK and TinyUSB
come from the configured host SDK. Use `./build.sh build-debug`. Do not
distribute UF2 files without the corresponding source and license notices.

Upstream SHA-256: `3a992a2e86612756e42fefb0668acc9b9c4c0538d4f9c77f86fa7f410fcb04aa`

The checked-in `src/logic_analyzer.c`, `src/inc/logic_analyzer.h` and
`src/pio/logic_analyzer.pio` are the small adapted portion used by this firmware.
They are intentionally kept with the project adapter because no other upstream
files are required at runtime.
