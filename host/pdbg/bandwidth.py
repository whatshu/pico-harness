"""Measure current PDBG framing throughput without changing Debug firmware.

bulk: repeat reads of ONE frozen capture (not continuous acquisition), and send
padded GET_INFO requests (current firmware ignores their data; not an API promise).
spi: use firmware/test/bandwidth-work via an already-running OpenOCD Tcl server.
Optional google-crc32c accelerates host checks without changing the wire protocol.
"""
import argparse
import binascii
import hashlib
import json
from pathlib import Path
import queue
import statistics
import struct
import subprocess
import threading
import time

from . import protocol
from .device import Probe
from .protocol import Command, Frame, HEADER, MAX_PAYLOAD, spi_test_pattern
from .worktest import OpenOCD


class Receiver:
    def __init__(self, elf, port):
        self.ocd = OpenOCD(port)
        self.elf = Path(elf).resolve()
        self.symbols = {}
        for line in subprocess.check_output(['arm-none-eabi-nm', self.elf], text=True).splitlines():
            fields = line.split()
            if len(fields) == 3:
                self.symbols[fields[2]] = int(fields[0], 16)
        self.address = self.symbols['bandwidth']

    def load(self):
        self.ocd.command('reset halt')
        self.ocd.command(f'load_image {{{self.elf}}}')
        self.ocd.command(f'verify_image {{{self.elf}}}')
        self.ocd.command(f'resume 0x{self.symbols["_entry_point"]:x}')
        self.wait(lambda: self.words()[0] == 0x42575431)

    @staticmethod
    def wait(predicate):
        deadline = time.monotonic() + 2
        while not predicate():
            if time.monotonic() > deadline:
                raise TimeoutError('Work receiver did not acknowledge')
            time.sleep(.005)

    def words(self):
        return self.ocd.read(self.address, 16)

    def configure(self, length):
        sequence = self.words()[1] + 1
        self.ocd.write(self.address + 12, [length])
        self.ocd.write(self.address + 4, [sequence])
        self.wait(lambda: self.words()[2] == sequence)
        assert not self.words()[15], 'Work rejected configuration'


def transfer_run(probe, command, payloads, expected, seconds, depth, useful_bytes):
    """Bounded producer; validate CRC, command, sequence, status and response size.

    Timing includes framing, checks, thread startup and final drain. No SWD polls
    or file writes occur in the interval. Only application payload counts as useful.
    """
    pending = queue.Queue()
    slots = threading.Semaphore(depth)
    stop = threading.Event()
    start = time.perf_counter()
    deadline = start + seconds
    durations = []
    buckets = []
    blocks = 0
    wire_out = wire_in = 0
    bucket_start = start
    bucket_blocks = 0
    receive_buffer = bytearray()
    usb_reads = 0

    def writer():
        index = 0
        try:
            while time.perf_counter() < deadline and not stop.is_set():
                if not slots.acquire(timeout=.1):
                    continue
                if stop.is_set() or time.perf_counter() >= deadline:
                    slots.release()
                    break
                seq = next(probe._sequence)
                payload = payloads[index % len(payloads)]
                request = Frame(command, seq, payload).encode()
                sent = time.perf_counter()
                written = probe.endpoint_out.write(request, 3000)
                if written != len(request):
                    raise RuntimeError('short USB write')
                pending.put((seq, index, sent, len(request)))
                index += 1
        except BaseException as error:
            pending.put(error)
        finally:
            pending.put(None)

    thread = threading.Thread(target=writer, daemon=True)
    thread.start()
    try:
        while True:
            item = pending.get(timeout=5)
            if item is None:
                break
            if isinstance(item, BaseException):
                raise item
            seq, index, sent, out_size = item
            # USB bulk transfers need not have the same boundaries as PDBG
            # frames, particularly with multiple requests queued at once.
            while len(receive_buffer) < HEADER.size:
                receive_buffer.extend(probe.endpoint_in.read(16384, 3000))
                usb_reads += 1
            header = HEADER.unpack_from(receive_buffer)
            if header[0] != protocol.MAGIC or header[5] > MAX_PAYLOAD:
                raise ValueError('invalid response header')
            total = HEADER.size + header[5] + 4
            while len(receive_buffer) < total:
                receive_buffer.extend(probe.endpoint_in.read(16384, 3000))
                usb_reads += 1
            raw = bytes(receive_buffer[:total])
            del receive_buffer[:total]
            response = Frame.decode(raw)
            if response.command != (int(command) | 0x80) or response.sequence != seq:
                raise RuntimeError('response command/sequence mismatch')
            if not response.payload or response.payload[0]:
                raise RuntimeError(f'device status: {response.payload[:1].hex()}')
            expected(response.payload[1:], index)
            now = time.perf_counter()
            durations.append(now - sent)
            blocks += 1
            wire_out += out_size
            wire_in += len(raw)
            slots.release()
            if now - bucket_start >= 1:
                buckets.append((blocks - bucket_blocks) * useful_bytes / (now - bucket_start))
                bucket_start, bucket_blocks = now, blocks
    finally:
        stop.set()
        thread.join(timeout=4)
    elapsed = time.perf_counter() - start
    return dict(blocks=blocks, payload_bytes=blocks * useful_bytes, elapsed_seconds=elapsed,
                bytes_per_second=blocks * useful_bytes / elapsed,
                framed_usb_out_bytes=wire_out, framed_usb_in_bytes=wire_in,
                per_second_bytes=buckets,
                median_response_ms=statistics.median(durations) * 1000,
                max_response_ms=max(durations) * 1000, usb_reads=usb_reads,
                usb_protocol_errors=0)


def equal(actual, wanted):
    if actual != wanted:
        raise ValueError('response payload differs from baseline')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('bulk', 'spi', 'capture'))
    parser.add_argument('--serial', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--seconds', type=float, default=3)
    parser.add_argument('--crc', choices=('python', 'native'), default='python')
    parser.add_argument('--sizes', default='64,256,1024,2048,4095')
    parser.add_argument('--depths', default='1')
    parser.add_argument('--rates', default='1000000,4000000,8000000,12000000,16000000,20000000,24000000,30000000,40000000,62500000')
    parser.add_argument('--directions', default='in,out')
    parser.add_argument('--sample-rates', default='1000000,125000000')
    parser.add_argument('--elf')
    parser.add_argument('--port', type=int, default=6666)
    parser.add_argument('--load', action='store_true')
    args = parser.parse_args()
    sizes = [int(n) for n in args.sizes.split(',')]
    depths = [int(n) for n in args.depths.split(',')]
    if args.seconds <= 0 or any(not 4 <= n <= 4096 for n in sizes) or any(not 1 <= n <= 32 for n in depths):
        parser.error('seconds > 0, sizes 4..4096, depths 1..32 required')
    if args.mode == 'spi' and (not args.elf or max(sizes) > 4095):
        parser.error('SPI needs --elf and sizes <= 4095')
    if args.crc == 'native':
        import google_crc32c
        for data in (b'', b'123456789', bytes(range(256)), spi_test_pattern(4096)):
            assert protocol.crc32c(data) == google_crc32c.value(data)
            assert protocol.crc16_ccitt(data) == binascii.crc_hqx(data, 0xffff)
        protocol.crc32c = google_crc32c.value
        protocol.crc16_ccitt = lambda data: binascii.crc_hqx(data, 0xffff)
    probe = Probe.open(args.serial)
    receiver = Receiver(args.elf, args.port) if args.elf else None
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    result = dict(time=time.strftime('%Y-%m-%dT%H:%M:%S%z'), arguments=vars(args),
                  info=probe._ok(probe.command(Command.GET_INFO), 'info').rstrip(b'\0').decode(),
                  git_head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                  runs=[])
    if receiver:
        result['work_elf_sha256'] = hashlib.sha256(receiver.elf.read_bytes()).hexdigest()
    def save(entry):
        result['runs'].append(entry)
        output.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({k: v for k, v in entry.items() if k != 'per_second_bytes'}), flush=True)
    try:
        if receiver and args.load:
            receiver.load()
        if args.mode == 'capture':
            for frequency in map(int, args.sample_rates.split(',')):
                start = time.perf_counter()
                data = probe._ok(probe.command(Command.CAPTURE, struct.pack('<II', 32768, frequency)), 'capture')
                capture_id, count, elapsed_us = struct.unpack('<III', data)
                sample_hz, _, flags, _ = struct.unpack('<IIII', probe._ok(probe.command(Command.CAPTURE_STATUS), 'capture status'))
                read_start = time.perf_counter()
                raw = bytearray()
                for offset in range(0, count, 1023):
                    length = min(1023, count - offset)
                    payload = probe._ok(probe.command(Command.CAPTURE_READ,
                        struct.pack('<IIH', capture_id, offset, length)), 'read capture')
                    if len(payload) != length * 4:
                        raise ValueError('incorrect capture length')
                    raw.extend(payload)
                end = time.perf_counter()
                samples = struct.unpack(f'<{count}I', raw)
                edges = [i for i in range(1, count) if (samples[i] ^ samples[i - 1]) & (1 << 5)]
                widths = [b - a for a, b in zip(edges, edges[1:])]
                filename = output.with_name(output.stem + f'-{frequency}Hz.u32le')
                filename.write_bytes(raw)
                save(dict(requested_sample_hz=frequency, sample_hz=sample_hz, count=count,
                    device_elapsed_us=elapsed_us, flags=flags, upload_seconds=end - read_start,
                    total_seconds=end - start, upload_bytes_per_second=len(raw) / (end - read_start),
                    raw_file=str(filename), raw_sha256=hashlib.sha256(raw).hexdigest(),
                    gp5_edges=len(edges), gp5_min_half_period_samples=min(widths) if widths else None,
                    gp5_max_half_period_samples=max(widths) if widths else None,
                    gp5_observed_hz=len(edges) * sample_hz / (2 * (count - 1)),
                    passed=count == 32768 and flags & 14 == 0))
        elif args.mode == 'bulk':
            capture = probe._ok(probe.command(Command.CAPTURE, struct.pack('<II', 32768, 1000000)), 'capture')
            capture_id, count, elapsed = struct.unpack('<III', capture)
            result['frozen_capture'] = dict(id=capture_id, count=count, elapsed_us=elapsed,
                status=list(struct.unpack('<IIII', probe._ok(probe.command(Command.CAPTURE_STATUS), 'status'))))
            for direction in args.directions.split(','):
                for size in sizes:
                    if direction == 'in':
                        size = min(1023, size // 4) * 4
                        command = Command.CAPTURE_READ
                        payload = struct.pack('<IIH', capture_id, 0, size // 4)
                    elif direction == 'out':
                        command, payload = Command.GET_INFO, spi_test_pattern(size)
                    else:
                        parser.error('directions must be in,out')
                    baseline = probe._ok(probe.command(command, payload), 'baseline')
                    for depth in depths:
                        entry = dict(direction=direction, block_bytes=size, depth=depth)
                        entry.update(transfer_run(probe, command, [payload], lambda data, _: equal(data, baseline), args.seconds, depth, size))
                        entry['passed'] = True
                        save(entry)
        else:
            result['work_initial'] = receiver.words()
            for rate in map(int, args.rates.split(',')):
                actual_rate = probe.spi_config(rate, 0)
                for size in sizes:
                    # Sequence-dependent bytes detect dropped/duplicated blocks too.
                    base = spi_test_pattern(size)
                    payloads = [bytes((v + i * 17) & 255 for v in base) for i in range(256)]
                    for depth in depths:
                        receiver.configure(size)
                        def check_response(data, index):
                            if len(data) != size:
                                raise ValueError('wrong SPI response size')
                        entry = dict(direction='host_to_work_spi', requested_hz=rate,
                                     actual_hz=actual_rate, block_bytes=size, depth=depth)
                        entry.update(transfer_run(probe, Command.SPI_TRANSFER, payloads, check_response, args.seconds, depth, size))
                        # Drain receiver validation after the LAST block, outside timing.
                        time.sleep(.01)
                        words = receiver.words()
                        entry['work'] = dict(blocks=words[4], bytes=words[5], errors=words[6], ready=words[7],
                            first_bad_block=words[8], first_bad_index=words[9], expected=words[10], actual=words[11])
                        entry['passed'] = words[4] == entry['blocks'] and words[5] == entry['payload_bytes'] and words[6] == 0
                        save(entry)
                        receiver.configure(0)
    except Exception as error:
        result['failure'] = f'{type(error).__name__}: {error}'
        output.write_text(json.dumps(result, indent=2) + '\n')
        raise
    finally:
        if receiver:
            receiver.configure(0)
            receiver.ocd.socket.close()
        if args.mode == 'spi':
            probe.spi_config(1000000, 0)
        probe.close()


if __name__ == '__main__':
    main()
