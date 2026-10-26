#!/usr/bin/env python3
"""Verify finite board-RAM captures; temporarily drive Work GP10 at 5 kHz.

Requires pc1 probe firmware and harness-terminal on Work. Stops PWM on exit.
Captures finish entirely in device RAM before USB readout; no streaming mode.
"""
import argparse
from array import array
import base64
import json
from pathlib import Path
import statistics
import time
from phbridge.ipc import Client


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--quick', action='store_true')
    args = parser.parse_args()
    results = []
    with Client() as c:
        cursor = c.call('device.open', {'serial': args.serial})['cursor']
        assert ';pc1;' in c.call('board.info'), 'pc1 firmware required'
        original_power = c.call('board.status')['power_on']
        assert original_power, 'Work must already be powered'

        def command(text, expected):
            nonlocal cursor
            c.call('console.write', {'data': base64.b64encode((text+'\n').encode()).decode()})
            end = time.monotonic()+3
            output = b''
            while time.monotonic() < end:
                batch = c.call('events.read', {'cursor': cursor, 'limit': 128, 'wait_ms': 50})
                cursor = batch['cursor']
                for event in batch['events']:
                    if 'console' in event:
                        output += base64.b64decode(event['console'])
                if expected in output:
                    return
            raise AssertionError((text, output))

        try:
            command('pwm 10 5000 0.5', b'pwm: GP10 5000 Hz')
            # raw Debug pins: GP10=19, GP11=18, GP12=21, GP17=12, GP19=8.
            profiles = [
                (1, 1 << 10, 1310720),
                (2, (1 << 10) | (1 << 11), 655360),
                (4, (1 << 10) | (1 << 12), 327680),
                (8, (1 << 10) | (1 << 17), 163840),
                (16, (1 << 10) | (1 << 19), 81920),
                (32, 0, 40960),
            ]
            cases = [(bits, mask, 512 if args.quick else count, 0, 500000)
                     for bits, mask, count in profiles]
            cases += [(bits, mask, 512 if args.quick else count, edge, 500000)
                      for bits, mask, count in [(8, 1 << 10, 163840),
                                                (16, (1 << 10) | (1 << 19), 81920), (32, 0, 40960)]
                      for edge in (1, 2)]
            if not args.quick:
                cases += [(1, 1 << 10, 1310720, 0, 100000),
                          (1, 1 << 10, 1310720, 0, 50000000),
                          (32, 0, 40960, 0, 2500)]
            for bits, mask, count, edge, rate in cases:
                frequency = 50 if rate < 100000 else 5000
                if frequency != 5000:
                    command(f'pwm 10 {frequency} 0.5', f'pwm: GP10 {frequency} Hz'.encode())
                started = time.monotonic()
                capture = c.call('logic.start', {'count': count, 'rate': rate, 'trigger': edge,
                                                'pin': 10, 'timeout_ms': 1000, 'channels': mask})
                deadline = started + count/rate + count*4/100000 + 10
                while True:
                    status = c.call('logic.status', {'capture_id': capture})
                    if status is not None:
                        break
                    assert time.monotonic() < deadline, 'finite capture did not complete'
                    # Prove that ordinary board queries remain responsive.
                    c.call('console.status')
                    time.sleep(.02)
                ready_seconds = time.monotonic() - started
                assert status['count'] == count, status
                samples = array('I')
                while len(samples) < count:
                    limit = min(32768, count-len(samples))
                    page = c.call('logic.read', {'capture_id': capture, 'offset': len(samples), 'limit': limit})
                    assert len(page['samples']) == limit and page['capture_id'] == capture
                    samples.extend(page['samples'])
                if mask:
                    assert all(not (sample & ~mask) for sample in samples), 'unselected GPIO leaked into capture'
                levels = [bool(s & (1 << 10)) for s in samples]
                rises = [i for i in range(1, count) if levels[i] and not levels[i-1]]
                if rate >= frequency * 4:
                    periods = [b-a for a, b in zip(rises, rises[1:])]
                    assert periods, 'PWM transitions missing'
                    expected = rate/frequency
                    assert abs(statistics.median(periods)-expected) <= 1, (bits, edge, periods[:10])
                    if any(abs(period - expected) > 1 for period in periods):
                        anomalies = [(rises[i], period) for i, period in enumerate(periods) if abs(period-expected) > 1]
                        failure = dict(bits=bits, count=count, rate=rate, median=statistics.median(periods),
                                       minimum=min(periods), maximum=max(periods), anomalies=anomalies[:100],
                                       status=status, ready_seconds=ready_seconds)
                        Path(args.output).write_text(json.dumps(failure, indent=2))
                        Path(args.output+'.samples.bin').write_bytes(samples.tobytes())
                        raise AssertionError(failure)
                if edge:
                    index = status['trigger_index']
                    assert abs(index-count//4) <= 2
                    assert levels[index] == (edge == 1) and levels[index-1] != levels[index]
                result = dict(storage_bits=bits, mask=mask, count=count, edge=edge, rate=rate,
                              duration_seconds=count/rate, ready_seconds=ready_seconds,
                              total_seconds=time.monotonic()-started, rising_edges=len(rises),
                              trigger_index=status['trigger_index'],
                              period_min=min(periods), period_max=max(periods))
                results.append(result)
                Path(args.output).write_text(json.dumps(results, indent=2))
                print(json.dumps(result), flush=True)
                c.call('logic.cancel')
        finally:
            c.call('logic.cancel')
            command('pwm 10 off', b'pwm: GP10 stopped')
            assert c.call('board.status')['power_on'] == original_power
    print(f'PASS: {len(results)} finite board-memory captures; PWM stopped; power unchanged')


if __name__ == '__main__':
    main()
