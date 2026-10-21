**English** · [简体中文](zh-CN/probe-core-performance.md)

# Debug firmware performance validation

On 2026-10-21, the optimized Debug firmware was compared with commit `56aaa45`
on RP2040 probe `E661AC8863787926`, using the same Linux host, USB connection,
existing Work `harness-terminal` and `RelWithDebInfo` builds. The benchmark
owns CDC and vendor USB directly; `phd` was stopped. Work firmware was not
programmed. Work power started off and was restored off.

## Measured results

The primary comparison used 20 transfers/captures per case and 100 idle
control requests. Control queries ran continuously during each 16 KiB Work
SPI transfer: 19,384 queries before and 22,503 after. Timings include host and
device round trips. Capture time includes acquisition and all 128 KiB of
readout, at a requested 1 MHz with no trigger.

| Measurement | Baseline | Optimized |
| --- | ---: | ---: |
| Debug Bulk 4096 B, median | 12.692 ms | 9.862 ms |
| Debug Bulk 64 B, median | 1.216 ms | 1.227 ms |
| Idle GetInfo, median | 0.661 ms | 0.660 ms |
| Control during Work SPI, median | 0.703 ms | 0.748 ms |
| Control during Work SPI, p99 | 2.589 ms | 1.200 ms |
| Control during Work SPI, maximum | 20.870 ms | 4.314 ms |
| Work SPI 16 KiB, median | 889.172 ms | 889.815 ms |
| SUMP 32,768 samples, median of successful captures | 671.938 ms | 326.365 ms |
| Complete SUMP captures within 1 second | 13/20 | 20/20 |

The 4 KiB Bulk latency fell 22.3%, capture/readout time fell 51.4% and the
observed maximum control latency during SPI fell 79.3%. These are measured
run statistics, not latency guarantees. Work SPI total throughput, small Bulk
and idle control latency have no claimed improvement. Every successful Bulk
completion matched the host's CRC-32C; Work-route transfers also checked the
CRC16 returned by Work for each block.

Earlier A/B/A checks on the same fixture also showed the effect: 4 KiB Bulk
medians 12.28 → 9.88 → 12.63 ms; successful capture medians
671 → 289 → 672 ms. The baseline lost 1/10 and 3/20 captures in those runs;
the first optimized run completed 15/15. This variation is why the final
table reports its own sample counts and timings.

## Changes and correctness checks

- Bulk reuses the existing table-driven CRC implementation with an incremental
  interface. Ring CRC/copy operations process at most two contiguous spans.
- SPI DMA starts and returns to the Core 1 loop. Later polls finish the
  transfer, leaving UART, PH control and capture completion serviceable.
  CRC replies arriving before the DMA completion poll are retained.
- Only the SUMP worker consumes capture completion. The previous extra outer
  poll could consume completion without publishing a SUMP result.
- SUMP writes up to 64 bytes at once and explicitly flushes the final short
  packet. ID replies remain queued under backpressure; USB servicing never
  waits for its own completion callback.
- Bulk reset stops the USB producer and requests cleanup on Core 1. DMA is
  cancelled before buffers/ring positions can be reused.

Production C code passed 22 host-model checks across RP2040/RP2350 configurations:
pending DMA, shift-register completion, overlap rejection, timeout/power
loss, early UART CRC, multiple wrapped SPI blocks, reset ownership, CRC
rejection, SUMP backpressure, channel order and zero padding. Both board
variants built successfully; only RP2040 was tested on hardware.

Additional hardware regressions completed 70 Debug Bulk transfers at packet
and ring boundaries, rejected an intentionally wrong CRC and recovered,
completed 18 Work SPI transfers at 128/2048/2049/4096/5000/16384 bytes, and
returned all 15 captures at 4/20/64/1028/32768 samples. Three SPI transfers
were interrupted after observing active CS; each failed explicitly, Debug
Bulk remained usable, and Work SPI recovered after the existing prepare
lease expired.

The release `phd` passed all five `test_ph_device.py` hardware tests, 270
sequential/concurrent/mixed control queries, 60 Debug Bulk transfers and 10
connection cycles. Read-only SWD block/scalar comparisons and target FAULT
recovery passed. Two Console subscribers received identical replies to six
terminal commands. These compatibility checks used a separate isolated
daemon instance, stopped after validation.

A brief rail-disable interval left Work near 3.3 V. Retrying after 1.1 seconds
could fail because the old Work receive transaction remained prepared for
up to 2 seconds. Waiting 2.2 seconds passed 10 repeated interruption/recovery
cycles. Holding power off for 200 ms brought Work to 30 mV and a subsequent
1.1-second startup wait also recovered. The benchmark observes this existing
protocol limit; it does not promise immediate recovery from a brief power cut.

The optimized ELF's `size` text grew by 1568 bytes and BSS by 16 bytes.
There is no dynamic allocation in the new paths. USB-reset cancellation is
covered by the host model; physical unplug/reset during DMA and RP2350
hardware remain outside this validation.

## Reproduce

Use a dedicated fixture. Stop `phd` before the raw benchmark and select the
probe serial explicitly. Build each revision separately; preserve the
baseline image before switching source revisions. For RP2040:

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

Program the intended Debug image before each measurement. The benchmark
itself never flashes firmware. Work SPI requires `harness-terminal` already
installed; its workload sends unknown console commands. Omit `--work-spi`
and/or `--la` when those fixture operations are unwanted. JSON includes raw
latencies, capture failures and power-restoration state; a failed workload
returns nonzero and saves partial results.

Local evidence is in ignored `BUILD/probe-perf/`: `baseline-final.json`,
`optimized-final.json`, `regression-confirmed.json` and `comparison.json`.
Daemon compatibility evidence is `phd-compatibility.json` and `phd-device-tests.log`.
The original full-flash backup is `original.uf2`. SHA-256 of the compared images:

```text
baseline  5e46ad41a7a2709ea2c36b4b6af0f48fb7a5506f39a83124e24f9296472201bb
optimized 98ea6862e4df99ea0f9ad0c0892723bc36fd88a6cb8f6c3073fcc3f084c5f2cf
```
