# Finite logic capture memory limits

The RP2040 probe reserves **160 KiB** of its RAM for logic samples. Acquisition
finishes in board RAM before USB readout begins. There is no live streaming,
downsampling or compression based on signal activity. Duration is the selected
profile's maximum point count divided by sample rate, rather than a fixed number
of seconds.

The previous buffer held 32768 full-width points (128 KiB). Keeping the existing
RTOS heap, UART trace buffers and other services, the new image links with 250176
bytes of BSS. The sample buffer is 163840 bytes. This is the implemented budget
for the current combined probe firmware, rather than a claim that all chip RAM
can be dedicated to logic capture. Pico 2 has not been capacity-benchmarked.

## Capacity

| Selection, immediate capture | Points | 500 kS/s | 100 kS/s | 2.5 kS/s |
| --- | ---: | ---: | ---: | ---: |
| All 16 connected channels | 40960 | 81.92 ms | 409.6 ms | 16.384 s |
| Work GP12 + GP13 | 655360 | 1.31072 s | 6.5536 s | 262.144 s |
| One connected channel | 1310720 | 2.62144 s | 13.1072 s | 524.288 s |

These durations follow from memory capacity and clock configuration. The longest
single-channel duration of 524.288 seconds is calculated, not a completed
524-second hardware run. Supported presets range from 2.5 kS/s to 50 MS/s.

PIO stores the span of selected physical Debug GPIOs, rounded up to
1/2/4/8/16/32 bits. Thus two widely separated physical pins may require more space
than GP12 + GP13. The desktop Channels selection controls acquisition and displays
the resulting maximum duration. Disabled channels are omitted; a trigger pin is
included even when hidden. With rising/falling triggers, DMA stores at least one
byte per point: single-channel capacity becomes 163840 points (327.68 ms at
500 kS/s, 1.6384 s at 100 kS/s). Triggered 16/32-bit profiles hold 81920/40960 points.
This retains the quarter-window pretrigger history and existing edge timing.

## Hardware validation

Device: RP2040 probe `YOUR_SERIAL`; Work `harness-terminal`; temporary Work
GP10 PWM, disabled after validation. The final `pc1` firmware image SHA256 is
`ef6bd4494c6a6443c4b89c110e8722d2ab877d1baa2e21bf05976ba5c69022fb`.

Run `PYTHONPATH=host python3 host/tools/check_logic_memory.py --serial SERIAL
--output RESULTS.json` as one command. This requires a matching `pc1` probe,
current phd, and harness-terminal Work. The tool briefly configures Work GP10 PWM
and restores it to off in cleanup.

All 15 full-buffer cases passed: immediate profiles with 1/2/4/8/16/32-bit storage,
rising and falling triggers with 8/16/32-bit storage, single-channel 1310720-point
capture at 100 kS/s and 50 MS/s, and all-channel 40960-point capture at 2.5 kS/s.
The checks cover chronological sample order, PWM periods, trigger position,
selected-channel masks and paged RPC readback. The production adapter also checks
completion flags, actual count/rate and CRC32C. Board status remained responsive
while sampling. Raw case metrics are in the local build artifact
`BUILD/logic-memory-validation/full.json`.

At 500 kS/s, the full single-channel record takes 2.62144 s to acquire and became
ready in 3.203 s including USB readout. At 100 kS/s, acquisition took 13.1072 s
and became ready in 13.695 s. All-channel 2.5 kS/s acquisition took 16.384 s and
became ready in 16.963 s. Packed readout transfers at most 160 KiB instead of
expanding single-channel data to 5 MiB on USB.

The actual desktop worker additionally passed legacy captures, a 1310720-point
single-channel record, a 16.384-second all-channel record, and cancellation of a
524.288-second configured record followed by a successful restart. Offline GUI
coverage verifies channel masks, duration units and memory-derived limits.
Firmware transport tests verify that sample data is withheld until acquisition
completes. Legacy SUMP four-byte readout remains available; selected/large records
require the `pc1` firmware tag.
