#!/usr/bin/env python3
"""Measure SUMP sampling accuracy and input-frequency limits with Work PWM.

Requires a dedicated RP2040 Debug and harness-terminal Work fixture. Stop phd
first. No firmware is programmed. PWM on the selected carrier pin is stopped
after the run and the original Work power state is restored.
"""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import re
import struct
import time

import serial

from tools.check_probe_core import Probe

CHANNELS = {5: 3, 10: 7, 26: 2}
SAMPLES = 32768
DEBUG_CLOCK_HZ = 125000000  # Measured firmware build; no clock change is made.


class SignalProbe(Probe):
    def __init__(self, serial_number):
        self.console = bytearray()
        super().__init__(serial_number)

    def next_message(self):
        message = super().next_message()
        if message is not None and message.HasField('console_data'):
            self.console.extend(message.console_data.data)
        return message

    def command(self, line, expected):
        self.console.clear()
        self.request('console_tx', data=(line + '\n').encode())
        if expected not in self.console or not self.console.endswith(b'harness> '):
            self.receive(lambda _: expected in self.console and self.console.endswith(b'harness> '))
        result = self.console.decode(errors='replace')
        if 'error:' in result:
            raise RuntimeError(result)
        return result

    def pwm(self, pin, frequency):
        reply = self.command(f'pwm {pin} {frequency} 0.5', f'pwm: GP{pin}'.encode())
        return int(re.search(r'actual (\d+) Hz', reply)[1])


def sample_rate(divider):
    requested = 100000000 // (divider + 1)
    # Match the firmware's integer 16.8 divider rounding.
    div256 = (DEBUG_CLOCK_HZ * 256 + requested - 1) // requested
    return DEBUG_CLOCK_HZ * 256 / div256


def capture(port, divider):
    port.write(bytes(5))
    time.sleep(.01)
    port.reset_input_buffer()
    commands = (b'\x80' + struct.pack('<I', divider)
                + b'\x81' + struct.pack('<I', SAMPLES // 4)
                + b'\xc0' + bytes(4) + b'\xc1' + bytes(4) + b'\x01')
    started = time.perf_counter()
    port.write(commands)
    data = bytearray()
    deadline = time.monotonic() + SAMPLES / sample_rate(divider) + 3
    while len(data) < SAMPLES * 4 and time.monotonic() < deadline:
        data.extend(port.read(min(16384, SAMPLES * 4 - len(data))))
    if len(data) != SAMPLES * 4:
        raise TimeoutError(f'capture returned {len(data)}/{SAMPLES * 4} bytes')
    return bytes(data), (time.perf_counter() - started) * 1000


def analyze(data, fs, reference, channel):
    words = [word for word, in struct.iter_unpack('<I', data)]
    if any(word >> 16 for word in words):
        raise AssertionError('nonzero unused SUMP channels')
    bits = [(word >> channel) & 1 for word in words]
    rises = [i for i in range(1, len(bits)) if bits[i] and not bits[i - 1]]
    edges = [i for i in range(1, len(bits)) if bits[i] != bits[i - 1]]
    periods = Counter(b - a for a, b in zip(rises, rises[1:]))
    frequency = (len(rises) - 1) * fs / (rises[-1] - rises[0]) if len(rises) > 1 else 0
    error = (frequency / reference - 1) * 100
    return {
        'sample_hz': fs, 'reference_hz': reference, 'measured_hz': frequency,
        'error_pct': error, 'rising_edges': len(rises),
        # Meaningful as a sampling-rate estimate only with a low-frequency
        # reference. High-frequency aliases must not be used to infer a rate.
        'inferred_effective_sample_hz': reference * (rises[-1] - rises[0]) / (len(rises) - 1) if len(rises) > 1 else 0,
        'high_fraction': sum(bits) / len(bits),
        'run_samples': dict(sorted(Counter(b - a for a, b in zip(edges, edges[1:])).items())),
        'period_samples': dict(sorted(periods.items())),
        'frequency_ok': abs(error) <= .2,
        'period_ok': bool(periods) and max(periods) <= math.ceil(fs / reference) + 1,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--pin', type=int, choices=CHANNELS, default=5)
    parser.add_argument('--repeats', type=int, default=5)
    parser.add_argument('--divider', type=int, default=1, help='SUMP divider, default 1 = 50 MS/s')
    parser.add_argument('--rates', action='store_true', help='also calibrate sampling presets with a 100 kHz reference')
    parser.add_argument('--signal-hz', type=int, nargs='+', default=[1000000, 5000000, 12500000, 15625000, 17857143, 20833333, 25000000, 31250000])
    args = parser.parse_args()
    if args.repeats < 1 or not 0 <= args.divider <= 39999 or min(args.signal_hz) < 1:
        parser.error('invalid repeats, divider or signal frequency')
    report = {
        'ok': False, 'started_utc': datetime.now(timezone.utc).isoformat(),
        'serial': args.serial, 'pin': args.pin, 'sump_channel': CHANNELS[args.pin],
        'debug_clock_hz': DEBUG_CLOCK_HZ, 'samples': SAMPLES,
        'mode': 'immediate; 16 channels; input synchronizer enabled',
        'rate_sweep': [], 'signal_sweep': [],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    raw = args.output.with_suffix('') / 'raw'
    raw.mkdir(parents=True, exist_ok=True)
    probe = None
    initial_power = None

    def save():
        args.output.write_text(json.dumps(report, indent=2) + '\n')

    def measure(port, key, divider, reference, requested):
        rows = []
        for iteration in range(args.repeats):
            data, elapsed = capture(port, divider)
            filename = raw / f'{key}-d{divider}-f{requested}-n{iteration}.bin'
            filename.write_bytes(data)
            result = analyze(data, sample_rate(divider), reference, CHANNELS[args.pin])
            result.update(divider=divider, requested_signal_hz=requested,
                          iteration=iteration, readout_ms=elapsed, raw_file=str(filename))
            report[key].append(result)
            rows.append(result)
            save()
        passed = sum(row['frequency_ok'] and row['period_ok'] for row in rows)
        print(f'{key}: divider={divider}, input={reference} Hz, passed={passed}/{len(rows)}, '
              f'measured={min(row["measured_hz"] for row in rows):.1f}..'
              f'{max(row["measured_hz"] for row in rows):.1f} Hz', flush=True)

    try:
        probe = SignalProbe(args.serial)
        initial_power = probe.request('get_status').get_status.power_enabled
        report['initial_power'] = initial_power
        report['debug_info'] = probe.request('get_info').get_info.info
        probe.request('power_set', enabled=True)
        time.sleep(1.1)
        report['work_info'] = probe.command('info', b'firmware: harness-terminal')
        with serial.Serial(probe.la_port, timeout=.01, exclusive=True) as port:
            try:
                if args.rates:
                    reference = probe.pwm(args.pin, 100000)
                    for divider in (99, 49, 19, 9, 4, 3, 2, 1, 0):
                        measure(port, 'rate_sweep', divider, reference, 100000)
                for requested in args.signal_hz:
                    reference = probe.pwm(args.pin, requested)
                    measure(port, 'signal_sweep', args.divider, reference, requested)
            finally:
                port.write(bytes(5))
        # ok means the measurement completed, including expected failing
        # frequency points. Inspect frequency_ok/period_ok for each capture.
        report['ok'] = True
    except BaseException as error:
        report['error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        try:
            if probe is not None:
                try:
                    if initial_power is not None:
                        try:
                            probe.command(f'pwm {args.pin} off', f'pwm: GP{args.pin} stopped'.encode())
                        finally:
                            probe.request('power_set', enabled=initial_power)
                            report['power_restored'] = True
                finally:
                    probe.close()
        except Exception as error:
            report['ok'] = False
            report['cleanup_error'] = f'{type(error).__name__}: {error}'
        save()
    if not report['ok']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
