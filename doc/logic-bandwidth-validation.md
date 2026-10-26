**English** · [简体中文](zh-CN/logic-bandwidth-validation.md)

# Logic analyzer frequency measurements

On 2026-10-21, the current RP2040 firmware's highest validated SUMP preset
was **50 MS/s**. The highest input frequency that passed all repeated checks
on three carrier paths was **15.625 MHz**. Higher frequencies sometimes
appeared correct, but also lost pulses or aliased; they were not reliable
across capture phases.

## Fixture and pass criteria

- Debug serial `YOUR_SERIAL`, existing optimized firmware, default
  125 MHz clock; no firmware or clock configuration changes.
- Work's existing `harness-terminal` generated PWM on GP5, GP10 and GP26,
  observed as SUMP channels 3, 7 and 2 respectively. Work reported a 125 MHz
  clock. The reference was its reported actual PWM frequency, not the
  requested frequency, which can round to a different period.
- Immediate capture, all 16 channels, input synchronization enabled,
  32,768 samples per capture. `phd` stopped; direct LA CDC access.
- A capture passes if the frequency calculated from rising-edge spacing
  differs from the reference by at most 0.2%, and no rising-edge gap exceeds
  `ceil(sample_rate / reference_frequency) + 1` samples. Full data length and
  unused upper channel bits are also checked.

This measures digital edge/frequency capture on the connected carrier. It is
not an analog bandwidth measurement or an externally calibrated timebase.
Passing frequency checks does not establish precise pulse-width or duty
measurements near the limit.

## Sampling rate

The calibration used a 100 kHz PWM reference, five captures per preset.

| SUMP requested rate | Result |
| --- | --- |
| 1, 2, 5, 10, 20, 25 MS/s | Reference frequency correct, 5/5 each |
| 33.333 MS/s | PIO divider gives 33.299 MS/s; reference correct, 5/5 |
| 50 MS/s | Reference correct, 5/5 |
| 100 MS/s | Reference reads as 160 kHz, 0/5 |

At the 100 MS/s setting the 100 kHz reference occupies about 625 samples per
period, rather than 1000. This implies an average effective rate near
62.5 MS/s, while the client time axis still assumes 100 MS/s. An independent
1 MHz reference also read near 1.6 MHz. Returning all 32,768 samples therefore
does not establish correct acquisition timing.

The behavior is consistent with a PIO/DMA throughput limit causing sampling
stalls. SUMP does not expose the sampler's error flags, so the test establishes
the timing error, rather than directly diagnosing RXSTALL. The current SUMP
divider choices jump from 50 to 100 MS/s; 62.5 MS/s is not an available preset.

## Input frequency at 50 MS/s

Each boundary point was captured five times on each of the three paths.

| Actual PWM input | Passed captures across GP5/GP10/GP26 |
| --- | ---: |
| 12.500 MHz | 15/15 |
| 15.625 MHz | 15/15 |
| 17.857 MHz | 2/15 |
| 20.833 MHz | 6/15 |
| 25.000 MHz | 9/15 |
| 31.250 MHz | 0/15 |

A separate confirmation at 15.625 MHz completed **30/30 on each path,
90/90 total**. Observed frequency was 15.624809–15.625382 MHz; maximum
relative error was 0.002442%. Rising-edge intervals were 2–4 samples, with
no long gaps. This input uses an exact 50% PWM configuration (8 Work clock
cycles per period); odd PWM periods at other frequencies round the duty.

The non-monotonic pass counts at higher frequencies reflect phase-dependent
sampling. Some 25 MHz captures read exactly 25 MHz, while others became
constant high or lost many periods. They cannot be counted as a reliable
upper limit. At 15.625 MHz, measured high fraction ranged from 50% to 62.5%,
illustrating the limited pulse-width resolution despite correct frequency.

These are finite tests of three paths, with gaps between captures. They do
not establish a precise cutoff between 15.625 and 17.857 MHz, validate every
carrier pin, edge-triggered capture, RP2350 or long-duration operation.

## Reproduce and evidence

Stop `phd` and use a dedicated fixture with Work already running
`harness-terminal`. The tool stops the selected PWM and restores initial
Work power; it does not flash firmware. Select the intended serial:

```sh
PYTHONPATH=host python3 host/tools/check_logic_bandwidth.py \
  --serial YOUR_SERIAL --pin 5 --rates --repeats 5 \
  --signal-hz 12500000 15625000 17857143 20833333 25000000 31250000 \
  --output BUILD/logic-bandwidth/gp5.json
```

Repeat the signal scan with `--pin 10` and `--pin 26`. For the confirmation,
use `--signal-hz 15625000 --repeats 30` on each pin. Default SUMP divider is
1 (50 MS/s); `--divider 0` tests the inaccurate 100 MS/s setting explicitly.

JSON contains per-capture frequency, duty, edge/run histograms and raw-file
paths. Binary files preserve all 32-bit little-endian sample words. `ok`
means the measurement completed; inspect `frequency_ok` and `period_ok` for
each point, since failed high-frequency points are expected in a limit sweep.

Local evidence is under ignored `BUILD/logic-bandwidth/`: `sweep-50m.json`,
`boundary.json`, `confirmed.json`, `reproduce-tool.json`, and raw waveforms
under `boundary/` and `reproduce-tool/raw/`. The original 1 MHz reference
cross-check is in `sweep-initial.json`. Work started off and was restored off;
all three test PWMs were stopped. Debug and Work firmware were not rewritten.
