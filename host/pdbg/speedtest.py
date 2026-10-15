"""RAM-only dualcore UART/SPI/capture rate sweep for the Work self-test image."""
import argparse
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import time
from .worktest import Fixture, DIRECT_PINS
from .protocol import spi_test_pattern


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--elf', required=True)
    p.add_argument('--serial', required=True)
    p.add_argument('--output', required=True)
    p.add_argument('--repeats', type=int, default=8)
    args = p.parse_args()
    f = Fixture(args.elf, args.serial)
    f.load()
    probe = f.probe
    diagnostic = f.symbols['work_speed']
    result = {'time': time.strftime('%Y-%m-%dT%H:%M:%S%z'), 'serial': args.serial,
              'fixture_sha256': hashlib.sha256(Path(args.elf).read_bytes()).hexdigest(),
              'repeats': args.repeats, 'uart': [], 'spi': [], 'logic': []}
    def save(section, entry):
        result[section].append(entry)
        Path(args.output).write_text(json.dumps(result, indent=2) + '\n')
        print(section, json.dumps(entry), flush=True)
    payload = spi_test_pattern()
    for baud in (115200, 1000000, 2000000, 3000000, 4000000, 5000000, 6000000, 7000000, 7812500):
        f.command(3, baud)
        actual = probe.uart_config(0, baud)
        e = {'requested_baud': baud, 'debug_baud': actual, 'work_baud': f.words()[12],
             'blocks': 0, 'missing_bytes': 0, 'mismatches': 0, 'work_errors': 0}
        elapsed = 0
        for _ in range(args.repeats):
            f.command(6)
            start = time.monotonic()
            probe.uart_write(0, payload)
            received = bytearray()
            deadline = start + max(.2, len(payload) * 30 / baud)
            while len(received) < len(payload) and time.monotonic() < deadline:
                received.extend(probe.uart_read(0))
            elapsed += time.monotonic() - start
            e['blocks'] += 1
            e['missing_bytes'] += max(0, len(payload) - len(received))
            e['mismatches'] += sum(a != b for a, b in zip(payload, received))
            e['work_errors'] |= f.ocd.read(diagnostic + 4)[0]
            if e['missing_bytes'] or e['mismatches'] or e['work_errors']:
                break
        e['debug_status'] = asdict(probe.uart_status(0))
        e['host_roundtrip_bytes_per_second'] = round(2 * e['blocks'] * len(payload) / elapsed)
        e['passed'] = not (e['missing_bytes'] or e['mismatches'] or e['work_errors'] or e['debug_status']['rx_overruns'])
        save('uart', e)
    f.command(3, 115200)
    probe.uart_config(0, 115200)
    for mode in range(4):
        for rate in (1000000, 4000000, 8000000, 12000000, 16000000, 20000000, 24000000, 30000000, 40000000, 62500000):
            actual = probe.spi_config(rate, mode)
            e = {'mode': mode, 'requested_hz': rate, 'actual_hz': actual, 'blocks': 0, 'miso_errors': 0, 'mosi_errors': 0, 'incomplete_blocks': 0}
            elapsed = 0
            for _ in range(args.repeats):
                f.command(4, mode)
                before = f.words()
                start = time.monotonic()
                received = probe.spi_transfer(payload)
                elapsed += time.monotonic() - start
                e['blocks'] += 1
                e['miso_errors'] += abs(len(received) - len(payload)) + sum(a != (b ^ 0xa5) for a, b in zip(received, payload))
                try:
                    f.wait(lambda: f.words()[9] == before[9] + 1, timeout=.15)
                except TimeoutError:
                    e['incomplete_blocks'] += 1
                e['mosi_errors'] += f.words()[10] - before[10]
                if e['miso_errors'] or e['mosi_errors'] or e['incomplete_blocks']:
                    break
            e['host_transfer_bytes_per_second_each_direction'] = round(e['blocks'] * len(payload) / elapsed)
            e['passed'] = not (e['miso_errors'] or e['mosi_errors'] or e['incomplete_blocks'])
            save('spi', e)
    for pin in DIRECT_PINS:
        for hz in (1000000, 5000000, 10000000, 20000000, 31250000, 41666666, 50000000, 62500000):
            f.command(8, (hz << 5) | pin)
            actual = f.ocd.read(diagnostic + 8)[0]
            start = time.monotonic()
            capture = probe.capture(32768, 125000000)
            elapsed = time.monotonic() - start
            bits = [bool(v & (1 << pin)) for v in capture.samples]
            edges = [i for i in range(1, len(bits)) if bits[i] != bits[i - 1]]
            widths = [b - a for a, b in zip(edges, edges[1:])]
            expected = 125000000 / (2 * actual)
            # Asynchronous clocks and the source fractional divider permit +/-1 sample.
            bad = sum(abs(width - expected) > 1.05 for width in widths)
            observed = len(edges) * 125000000 / (2 * (len(bits) - 1))
            e = {'work_pin': pin, 'requested_signal_hz': hz, 'signal_hz': actual,
                 'requested_sample_hz': 125000000, 'timed_sample_hz': round(capture.actual_frequency),
                 'edges': len(edges), 'observed_signal_hz_assuming_requested_sample_clock': round(observed),
                 'min_half_period_samples': min(widths) if widths else 0,
                 'max_half_period_samples': max(widths) if widths else 0,
                 'bad_half_periods': bad, 'host_bytes_per_second': round(4 * len(bits) / elapsed),
                 'passed': bool(widths) and bad == 0 and abs(observed / actual - 1) < .015}
            save('logic', e)
    f.command(7)
    f.command(1, 0)


if __name__ == '__main__':
    main()
