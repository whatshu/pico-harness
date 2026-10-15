# Test firmware

Test firmware lives under `firmware/test/<project>/`, one independent Pico SDK
project per directory. `firmware/harness-target/example/` is only for examples
that enable the communication library; plain projects do not go there.

| Project | Purpose |
| --- | --- |
| `blink` | Blinks the LED; no communication library |
| `uart-console` | Plain UART echo; no communication library |
| `bandwidth-work` | Standalone PIO/DMA SPI bandwidth receive fixture, RAM-only |
| `work-selftest` | Hardware self-test using the harness-target shared SPI engine |

Build from the repository root, for example:

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
cmake -S firmware/test/blink -B BUILD/blink \
  -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=Debug
cmake --build BUILD/blink -j
```

Plain UART and LED usage is described in the
[getting started guide](../harness-target/GETTING_STARTED.md), and the
bandwidth test in its [fixture notes](bandwidth-work/README.md).
`./build.sh test --offline` builds the Work self-test and runs offline checks.