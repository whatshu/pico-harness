#!/usr/bin/env python3
"""Measure the Debug firmware directly on a dedicated, idle USB fixture.

Stop phd before running: this tool owns PH CDC, LA CDC and vendor interface 3.
Optional Work SPI tests require harness-terminal. Work power is restored; no
firmware is programmed. Use identical arguments and fixture for A/B runs.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import statistics
import struct
import threading
import time

import serial
import serial.tools.list_ports
import usb.core
import usb.util

from phbridge import frame, ph_pb2 as pb
from tools.check_bridge import crc32c


class Probe:
    def __init__(self, serial_number: str):
        self.device = next(d for d in usb.core.find(find_all=True, idVendor=0x2e8a, idProduct=0x000c)
                           if d.serial_number == serial_number)
        ports = [p for p in serial.tools.list_ports.comports() if p.serial_number == serial_number]
        control = next(p.device for p in ports if p.location and p.location.endswith(':1.1'))
        self.la_port = next(p.device for p in ports if p.location and p.location.endswith(':1.4'))
        self.port = serial.Serial(control, timeout=.01, exclusive=True)
        try:
            usb.util.claim_interface(self.device, 3)
        except BaseException:
            self.port.close()
            usb.util.dispose_resources(self.device)
            raise
        self.buffer = bytearray()
        self.seq = 0
        self.done = None

    def next_message(self):
        # Preserve an incomplete multi-byte prefix across serial reads. Fail on
        # corrupt framing instead of silently discarding measurement evidence.
        length = 0
        for index, byte in enumerate(self.buffer[:5]):
            length |= (byte & 127) << (index * 7)
            if not byte & 128:
                if not 0 < length <= 512:
                    raise ValueError(f'invalid PH length: {length}')
                end = index + 1 + length
                if len(self.buffer) < end:
                    return None
                message = pb.PhMessage.FromString(bytes(self.buffer[index + 1:end]))
                del self.buffer[:end]
                return message
        if len(self.buffer) >= 5:
            raise ValueError('PH varint exceeds five bytes')
        return None

    def receive(self, predicate, timeout=3):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            while (message := self.next_message()) is not None:
                if predicate(message):
                    return message
                if message.HasField('bulk_tx_done'):
                    self.done = message.bulk_tx_done
            self.buffer.extend(self.port.read(max(1, self.port.in_waiting)))
        raise TimeoutError('PH response timed out')

    def request(self, kind, **fields):
        self.seq += 1
        message = pb.PhMessage(seq=self.seq)
        body = getattr(message, kind)
        body.SetInParent()
        for key, value in fields.items():
            setattr(body, key, value)
        self.port.write(frame(message))
        reply = self.receive(lambda m: m.seq == self.seq)
        if reply.status != pb.OK:
            raise RuntimeError(f'{kind} failed: {reply}')
        return reply

    def bulk(self, data: bytes, route=0, queries=False):
        self.done = None
        expected = crc32c(data)
        started = time.perf_counter()
        self.request('bulk_tx_begin', route=route, length=len(data), crc32c=expected)
        errors = []

        def write():
            try:
                written = self.device.write(5, data, timeout=10000)
                if written != len(data):
                    raise IOError(f'partial Bulk write: {written}/{len(data)}')
            except Exception as error:
                errors.append(error)

        latencies = []
        if queries:
            writer = threading.Thread(target=write)
            writer.start()
            try:
                deadline = time.monotonic() + 10
                while self.done is None and time.monotonic() < deadline and not errors:
                    before = time.perf_counter()
                    self.request('get_info')
                    latencies.append((time.perf_counter() - before) * 1000)
            finally:
                writer.join()
        else:
            write()
        if errors:
            raise errors[0]
        if self.done is None:
            self.done = self.receive(lambda m: m.HasField('bulk_tx_done'), 10).bulk_tx_done
        if not self.done.ok or self.done.crc_recv != expected:
            raise AssertionError(f'Bulk integrity mismatch: {self.done}')
        return (time.perf_counter() - started) * 1000, latencies

    def close(self):
        self.port.close()
        try:
            usb.util.release_interface(self.device, 3)
        finally:
            usb.util.dispose_resources(self.device)


def stats(samples):
    ordered = sorted(samples)
    return {
        'n': len(samples), 'median_ms': statistics.median(ordered),
        'p95_ms': ordered[int((len(ordered) - 1) * .95)],
        'p99_ms': ordered[int((len(ordered) - 1) * .99)], 'max_ms': ordered[-1],
        'samples_ms': samples,
    }


def logic_analyzer(probe, iterations):
    samples, failures = [], []
    with serial.Serial(probe.la_port, timeout=.01, exclusive=True) as la:
        try:
            for index in range(iterations):
                la.write(bytes(5))
                time.sleep(.005)
                la.reset_input_buffer()
                la.write(b'\x02')
                if la.read(4) != b'1ALS':
                    raise AssertionError('SUMP identity mismatch')
                commands = b'\x80' + struct.pack('<I', 99) + b'\x81' + struct.pack('<I', 8192) + b'\x01'
                started = time.perf_counter()
                la.write(commands)
                received = bytearray()
                deadline = time.monotonic() + 1
                while len(received) < 32768 * 4 and time.monotonic() < deadline:
                    received.extend(la.read(min(16384, 32768 * 4 - len(received))))
                if len(received) != 32768 * 4:
                    failures.append({'iteration': index, 'bytes': len(received)})
                else:
                    # SUMP emits 16 channels in 32-bit little-endian words.
                    if any(received[2::4]) or any(received[3::4]):
                        raise AssertionError('nonzero unused SUMP channels')
                    samples.append((time.perf_counter() - started) * 1000)
        finally:
            la.write(bytes(5))
    return {'success': stats(samples) if samples else None, 'failures': failures}


def regression(probe, work_spi, la_enabled):
    results = {}
    for size in (1, 63, 64, 65, 512, 3000, 4096):
        data = bytes((index * 73 + 41) & 255 for index in range(size))
        for _ in range(10):
            probe.bulk(data)
    results['debug_crc_checked'] = 70
    data = bytes(range(64))
    probe.done = None
    probe.request('bulk_tx_begin', route=0, length=len(data), crc32c=crc32c(data) ^ 1)
    probe.device.write(5, data, timeout=1000)
    done = probe.receive(lambda m: m.HasField('bulk_tx_done')).bulk_tx_done
    if done.ok or done.crc_recv != crc32c(data):
        raise AssertionError('incorrect CRC was not rejected')
    probe.bulk(data)
    results['crc_rejection_and_recovery'] = True
    if work_spi:
        for size in (128, 2048, 2049, 4096, 5000, 16384):
            data = (b'__probe_perf_no_such_command__\n' * 700)[:size]
            for _ in range(3):
                probe.bulk(data, 1)
        results['work_spi_crc_checked'] = 18
        disabled_voltages = []
        for _ in range(3):
            data = (b'__probe_perf_no_such_command__\n' * 100)[:2048]
            probe.done = None
            probe.request('bulk_tx_begin', route=1, length=len(data), crc32c=crc32c(data))
            probe.device.write(5, data, timeout=1000)
            deadline = time.monotonic() + 1
            while probe.request('gpio_diag', pin=9).gpio_diag.level:
                if time.monotonic() > deadline:
                    raise AssertionError('SPI CS never became active')
            # Observe active CS before cutting power, during a 2048-byte DMA.
            probe.request('power_set', enabled=False)
            done = probe.done
            if done is None:
                done = probe.receive(lambda m: m.HasField('bulk_tx_done')).bulk_tx_done
            if done.ok:
                raise AssertionError('interrupted Work SPI unexpectedly succeeded')
            probe.bulk(bytes(65))
            disabled_voltages.append(probe.request('get_status').get_status.vbus_mv)
            probe.request('power_set', enabled=True)
            # A brief power cut or external supply can keep Work alive. Its
            # incomplete prepared transfer has a 2-second lease; allow that to
            # expire before preparing the recovery block.
            time.sleep(2.2)
            if probe.request('term_status').term_status.mode != 1:
                raise AssertionError('Work harness did not recover after power loss')
            if not probe.request('gpio_diag', pin=9).gpio_diag.level:
                raise AssertionError('SPI CS was not released after cancellation')
            probe.bulk(data, 1)
        results['drive_disable_during_active_spi_and_recovery'] = 3
        results['work_voltage_while_disabled_mv'] = disabled_voltages
    if la_enabled:
        with serial.Serial(probe.la_port, timeout=.01, exclusive=True) as la:
            try:
                for count in (4, 20, 64, 1028, 32768):
                    for _ in range(3):
                        la.write(bytes(5))
                        time.sleep(.005)
                        la.reset_input_buffer()
                        la.write(b'\x80' + struct.pack('<I', 99) + b'\x81' + struct.pack('<I', count // 4) + b'\x01')
                        received = bytearray()
                        deadline = time.monotonic() + 1
                        while len(received) < count * 4 and time.monotonic() < deadline:
                            received.extend(la.read(count * 4 - len(received)))
                        if len(received) != count * 4 or any(received[2::4]) or any(received[3::4]):
                            raise AssertionError(f'SUMP {count} samples: invalid data/length {len(received)}')
            finally:
                la.write(bytes(5))
        results['la_complete_captures'] = 15
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--label', default='')
    parser.add_argument('--iterations', type=int, default=20)
    parser.add_argument('--work-spi', action='store_true')
    parser.add_argument('--la', action='store_true')
    parser.add_argument('--regression', action='store_true', help='also exercise packet/ring boundaries and SPI drive-disable recovery')
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error('--iterations must be positive')
    report = {'serial': args.serial, 'label': args.label, 'ok': False, 'results': {},
              'started_utc': datetime.now(timezone.utc).isoformat(), 'iterations': args.iterations}
    probe = None
    initial_power = None
    try:
        probe = Probe(args.serial)
        initial_power = probe.request('get_status').get_status.power_enabled
        report['initial_power'] = initial_power
        report['info'] = probe.request('get_info').get_info.info
        samples = []
        for _ in range(100):
            before = time.perf_counter()
            probe.request('get_info')
            samples.append((time.perf_counter() - before) * 1000)
        report['results']['control'] = stats(samples)
        for size in (64, 4096):
            data = bytes((index * 73 + 41) & 255 for index in range(size))
            probe.bulk(data)  # warm-up
            samples = [probe.bulk(data)[0] for _ in range(args.iterations)]
            result = stats(samples)
            report['results'][f'debug_bulk_{size}'] = result
            print(f'Debug Bulk {size}: median {result["median_ms"]:.3f} ms', flush=True)
        if args.work_spi:
            probe.request('power_set', enabled=True)
            time.sleep(1.1)
            if probe.request('term_status').term_status.mode != 1:
                raise RuntimeError('Work SPI requires a harness-terminal fixture')
            # Unknown console commands exercise the bridge without changing
            # Work GPIO or firmware. Keep these bytes identical across A/B runs.
            data = (b'__probe_perf_no_such_command__\n' * 700)[:16384]
            samples, queries = [], []
            for _ in range(args.iterations):
                elapsed, latencies = probe.bulk(data, 1, True)
                samples.append(elapsed)
                queries.extend(latencies)
            report['results']['work_spi_16384'] = stats(samples)
            report['results']['control_during_spi'] = stats(queries)
            print(f'Work SPI: {len(queries)} concurrent control requests', flush=True)
        if args.la:
            report['results']['la_32768'] = logic_analyzer(probe, args.iterations)
        if args.regression:
            report['regression'] = regression(probe, args.work_spi, args.la)
        report['ok'] = not report['results'].get('la_32768', {}).get('failures')
    except BaseException as error:
        report['error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        try:
            if probe is not None:
                try:
                    if initial_power is not None:
                        probe.request('power_set', enabled=initial_power)
                        report['power_restored'] = True
                finally:
                    probe.close()
        except Exception as error:
            report['ok'] = False
            report['cleanup_error'] = f'{type(error).__name__}: {error}'
        destination = Path(args.output)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(report, indent=2) + '\n')
        print(f'Results saved to {destination}; ok={report["ok"]}', flush=True)
    if not report['ok']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
