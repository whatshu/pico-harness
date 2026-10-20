**English** · [简体中文](zh-CN/bridge-hardware-validation.md)

# Linux bridge hardware validation

On 2026-10-20, the Rust `phd` release build passed the workloads below on a
physical RP2040 PicoHarness. Python clients used authenticated TCP IPC; `phd`
owned the USB/CDC and CMSIS-DAP connections. No firmware was programmed.

## Fixture

| Item | Configuration |
| --- | --- |
| Host | Linux, loopback TCP `127.0.0.1:44841` |
| Probe serial | `E661AC8863787926` |
| Debug firmware | `PicoHarness/6;dualcore;ph-terminal;ph-t-bulk;la-sump` |
| USB | Full Speed, 12 Mbit/s |
| Work firmware | Existing `harness-terminal` |
| Console | Harness mode, reported baud 115207 |
| SWD | RP2040 core 0, 1 MHz, negotiated DAP packet size 64 bytes |
| Work power | Initially off; restored off after every Work test |

## Results

Latencies include the Python socket call and physical-device round trip. The
before/after measurements used the same fixture; these are individual runs,
not guarantees for other hosts or firmware.

| Operation | Before optimization | Final release |
| --- | ---: | ---: |
| Debug Bulk, 64 bytes, median of 20 transfers | 5.99 ms | 1.31 ms |
| Debug Bulk, 4096 bytes, median of 20 transfers | 16.99 ms | 12.50 ms |
| SWD Flash read, 256 words, one read | 341.70 ms | 22.93 ms |

Persistent Bulk handles avoid repeated enumeration and interface claims.
CMSIS-DAP block transfers reduce the number of SWD USB round trips. The final
1024-word Flash read took 92.17 ms. Control throughput varied between runs; no
control throughput improvement is claimed.

| Workload | Observed result |
| --- | --- |
| Fixed control workload | 6800 sequential, concurrent and mixed queries; no failures |
| Control latency | Sequential median 0.81 ms, p99 3.78 ms; eight-client throughput 989 calls/s |
| Fixed Debug Bulk workload | 240 transfers at 1, 63, 64, 65, 512 and 4096 bytes, including concurrent queries; all CRC32C results matched |
| Connection churn | 100 independent socket connections; no failures |
| Continuous mixed load | 60.01 s, eight query clients and one 4 KiB Bulk sender: 26,951 queries and 3413 transfers; no failures |
| Mixed-load latency | Query p99 31.54 ms, maximum 271.70 ms; Bulk p99 20.32 ms |
| Resource sample during continuous load | Over 15.00 s: CPU 2.40% of one core, RSS 9088 KiB throughout, at most 13 threads |
| SWD integrity | Block/scalar Flash values matched, including reads crossing 1 KiB and 4 KiB boundaries |
| SWD error recovery | An unmapped-memory read returned a target FAULT; the next CPUID read succeeded |
| SWD ownership | Competing client rejected; closing the owner's socket released its lease |
| Console | 300 `help`/`info` commands; two subscribers received identical replies, 146,323 bytes each |
| Slow Console subscriber | Retained-history loss was explicitly reported with `gap: true`; active subscribers lost no history |
| Work SPI Bulk | Three transfers each at 128, 2048, 4096, 8192 and 16,384 bytes; per-block and overall CRC checks passed |
| USB reenumeration | Three software-triggered BOOTSEL/application cycles; same daemon instance and client socket, control and cached Bulk recovered in 1.49–1.60 s |
| Final hardware regression | All five `test_ph_device.py` tests passed after reenumeration |

The Work SPI payload consisted of an unknown terminal command, avoiding changes
to GPIO or firmware. The reenumeration test warmed the cached Bulk connection,
entered BOOTSEL, checked that application access failed explicitly, and used
`picotool reboot` on the exact USB bus/address found at the selected physical
port. It then verified identity and Bulk CRC without restarting `phd` or the
client. This tested USB disappearance and reappearance, without physically
unplugging the cable.

## Reproduce the main workload

Run from the repository root with a dedicated fixture already running the
firmware described above. Use an unused port and an isolated state directory:

```sh
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
export PHD_ADDR=127.0.0.1:44841
export PHD_STATE_DIR="$PWD/BUILD/bridge-hardware/state"
export PYTHONPATH=host
ph-bridge/target/release/phdcli daemon start
python3 host/tools/check_bridge.py --serial E661AC8863787926 \
  --output BUILD/bridge-hardware/run.json \
  --swd-rp2040 --console-terminal --console-iterations 300 --soak-seconds 60
PHD_HARDWARE_TEST=1 PHD_SERIAL=E661AC8863787926 \
  python3 -m unittest discover -s host/tests -p test_ph_device.py -v
ph-bridge/target/release/phdcli daemon stop
```

Replace the serial with the intended fixture. The [hardware tool](../host/tools/check_bridge.py)
restores Work power and saves partial evidence when a workload fails. The
main workload does not include the separate Work SPI, process-resource or
BOOTSEL measurements. Raw local records from this run are under the ignored
`BUILD/bridge-hardware/` directory: `baseline.json`, `optimized.json`,
`final.json`, `resources.json`, `work-spi.json` and `recovery.json`.

## Tested scope

This record covers one Linux host and one RP2040 board. The continuous run was
60 seconds and the resource sample was 15 seconds. It does not establish
long-duration leak freedom, physical cable-unplug recovery, full 1 MiB Work
Bulk transfers or high-baud Console throughput. Flash programming,
macOS/Windows and MCP integration were outside this hardware-validation pass.

Software regressions additionally cover fragmented/corrupt PH frames,
invalidated streams after timeouts, bounded event queues, large receive
bursts, DAP block-reply validation and real socket/PTY IPC behavior. See
[Build and test](desktop-testing.md) for those entry points.
