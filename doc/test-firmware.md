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
[getting started guide](getting-started.md), and the
[bandwidth receiver](#usb--spi-bandwidth-receiver) below.
`./build.sh test --offline` builds the Work self-test and runs offline checks.

## USB → SPI bandwidth receiver

This optional RAM-only fixture targets the dualcore carrier wiring. Select the
**actual Work chip** with `-DPICO_BOARD=pico` or `pico2`. It receives SPI mode 0
on Work GP18/20/21 using PIO and DMA, verifies every byte, and automatically
rearms after CS goes high. GP19 is an input with a pull-down; this fixture
tests MOSI only. GP5 outputs a 100 kHz reference for finite logic captures.

Each block is 1–4095 bytes. Byte `i` in block `b` is
`(((i * 73 + 41) ^ (i >> 3)) + b * 17) & 255`. The SWD mailbox counts completed
blocks, bytes and mismatches and records the first bad byte. Changing block
size through the mailbox resets these counters. Block preparation and all SWD
access happen outside the timed host transfer interval.

The host still receives a full SPI response for every request because that is
the current Debug protocol. This fixture does not turn it into a write-only
transport. Debug firmware is unchanged. Reset/power loss discards the fixture
and restores Work's existing flash/ROM program.

Example (RP2040 Work; run OpenOCD in a separate terminal):

```sh
cmake -S firmware/test/bandwidth-work -B BUILD/bandwidth-work -G Ninja \
  -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=Release
cmake --build BUILD/bandwidth-work
openocd -f interface/cmsis-dap.cfg \
  -c 'adapter serial YOUR_SERIAL; adapter speed 1000' \
  -f target/rp2040.cfg -c 'bindto 127.0.0.1; init'
```

Enable Work power with `script/pdbgctl.sh power on` before connecting SWD.
Use a virtual environment with `pyusb`; install `google-crc32c` for the optional
native CRC mode. From the project root:

```sh
PYTHONPATH=host python -m pdbg.bandwidth spi --serial YOUR_SERIAL \
  --elf BUILD/bandwidth-work/bandwidth_work.elf --load \
  --crc native --sizes 4095 --rates 31250000 --depths 1 --seconds 60 \
  --output BUILD/spi-bandwidth.json
PYTHONPATH=host python -m pdbg.bandwidth bulk --serial YOUR_SERIAL \
  --crc native --sizes 4096 --depths 1 --seconds 60 \
  --output BUILD/usb-bandwidth.json
```

`bulk` IN repeatedly reads one frozen capture; it **does not measure continuous
acquisition**. `bulk` OUT uses padded `GET_INFO` requests accepted by the current
firmware; this is a diagnostic sink, not a supported application command shape.
Depths greater than 1 exercise an experimental host queue and frame assembler.
The normal `Probe.command()` client sends one command at a time. The benchmark
always validates response CRCs, command, sequence, status and length; IN also
compares each block to its baseline. Timing includes queue startup and drain,
host framing and validation. MB/s in the report means 1,000,000 payload bytes/s.
Inspect each JSON entry's `passed` and Work counters: a sweep intentionally
continues through data-error points, so process exit alone is not a pass result.

For finite-capture timing and RX-stall checks while this fixture is running:

```sh
PYTHONPATH=host python -m pdbg.bandwidth capture --serial YOUR_SERIAL \
  --crc native --sample-rates 1000000,62500000,125000000 \
  --output BUILD/finite-capture.json
```

This writes the actual samples as `.u32le` files and reports the GP5 edge widths.
Its `passed` field checks count and capture status; inspect the reference signal
measurements separately. It does not establish continuity between captures.

The tool restores SPI to 1 MHz and disarms the receiver. After testing, shut
down OpenOCD and reset Work or restore its original power state.