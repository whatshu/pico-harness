"""Flash and test the dualcore Work RP2040 through an OpenOCD Tcl server."""
import argparse
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import socket
import struct
import subprocess
import time

from .device import Probe
from .protocol import Command, Measurement, decode_status, spi_test_pattern

DIRECT_PINS = (5, 10, 11, 14, 15, 16, 17, 26, 27, 28)
DIRECT_MASK = sum(1 << pin for pin in DIRECT_PINS)


class OpenOCD:
    def __init__(self, port=6666):
        self.socket = socket.create_connection(("127.0.0.1", port), timeout=10)
        self.socket.settimeout(30)

    def command(self, command):
        wrapped = 'set rc [catch {' + command + '} result]; format "%d:%s" $rc $result'
        self.socket.sendall(wrapped.encode() + b'\x1a')
        data = bytearray()
        while not data.endswith(b'\x1a'):
            chunk = self.socket.recv(65536)
            if not chunk:
                raise ConnectionError("OpenOCD disconnected")
            data.extend(chunk)
        code, result = data[:-1].decode().split(':', 1)
        if code != '0':
            raise RuntimeError(result)
        return result.strip()

    def read(self, address, count=1):
        return [int(value, 0) for value in self.command(
            f'read_memory 0x{address:x} 32 {count}').split()]

    def write(self, address, values):
        words = ' '.join(hex(v) for v in values)
        self.command(f'write_memory 0x{address:x} 32 {{{words}}}')


class Fixture:
    def __init__(self, elf, flash_elf, serial, port=6666):
        self.elf = Path(elf).resolve()
        self.flash_elf = Path(flash_elf).resolve()
        self.probe = Probe.open(serial)
        self.ocd = OpenOCD(port)
        self.symbols = {}
        for line in subprocess.check_output(['arm-none-eabi-nm', self.elf], text=True).splitlines():
            fields = line.split()
            if len(fields) == 3:
                self.symbols[fields[2]] = int(fields[0], 16)
        self.mailbox = self.symbols['work_test']

    def load(self):
        self.ocd.command(f'program {{{self.flash_elf}}} verify reset')
        self.ocd.command('reset halt')
        self.ocd.command(f'load_image {{{self.elf}}}')
        self.ocd.command(f'verify_image {{{self.elf}}}')
        # RP2040 pauses its timer while either core is halted by the debugger.
        self.ocd.command('targets rp2040.core1; resume; targets rp2040.core0')
        self.ocd.command(f'resume 0x{self.symbols["_entry_point"]:x}')
        self.wait(lambda: self.words()[0] == 0x56313254)
        assert self.words()[13] == DIRECT_MASK

    def words(self):
        return self.ocd.read(self.mailbox, 16)

    @staticmethod
    def wait(condition, timeout=2):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if condition():
                return
            time.sleep(.005)
        raise TimeoutError('Work fixture did not reach expected state')

    def command(self, command, argument=0):
        sequence = self.words()[3] + 1
        self.ocd.write(self.mailbox + 16, [command, argument])
        self.ocd.write(self.mailbox + 8, [sequence])
        self.wait(lambda: self.words()[3] == sequence)
        assert self.words()[15] == 0

    def gpio(self):
        return struct.unpack('<I', self.probe._ok(
            self.probe.command(Command.GPIO_READ), 'gpio'))[0]

    def status(self):
        return decode_status(self.probe.command(Command.GET_STATUS))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--elf', required=True)
    parser.add_argument('--flash-elf', required=True)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--port', type=int, default=6666)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    fixture = Fixture(args.elf, args.flash_elf, args.serial, args.port)
    probe, ocd = fixture.probe, fixture.ocd
    output = Path(args.output)
    results = {'serial': args.serial, 'time': time.strftime('%Y-%m-%dT%H:%M:%S%z'),
               'elf_sha256': hashlib.sha256(fixture.elf.read_bytes()).hexdigest(),
               'flash_elf_sha256': hashlib.sha256(fixture.flash_elf.read_bytes()).hexdigest(),
               'tests': []}

    def test(name, action):
        try:
            details = action()
            entry = {'name': name, 'passed': True, 'details': details}
        except Exception as error:
            entry = {'name': name, 'passed': False, 'error': f'{type(error).__name__}: {error}'}
        results['tests'].append(entry)
        output.write_text(json.dumps(results, indent=2) + '\n')
        print(json.dumps(entry), flush=True)
        return entry['passed']

    if not test('flash_verify_load_run_ram', fixture.load):
        raise SystemExit(1)

    def swd_memory():
        address = fixture.symbols['swd_scratch']
        values = [((i * 0x1234567) ^ 0xa5c35a3c) & 0xffffffff for i in range(1024)]
        ocd.write(address, values)
        assert ocd.read(address, len(values)) == values
        return {'bytes': len(values) * 4}
    test('swd_ram_read_write', swd_memory)

    def hardware_breakpoint():
        table = ocd.read(0x14)[0] & 0xffff
        words = [int(v, 0) for v in ocd.command(f'read_memory {table} 16 36').split()]
        functions = dict(zip(words[::2], words[1::2]))
        address = functions[0x534d] & ~1  # RP2040 ROM memset ("MS").
        ocd.command('halt')
        ocd.command(f'bp {address} 2 hw')
        sequence = fixture.words()[3] + 1
        ocd.write(fixture.mailbox + 16, [5, 0x5a])
        ocd.write(fixture.mailbox + 8, [sequence])
        try:
            ocd.command('resume; wait_halt 2000')
            stopped = ocd.command('capture {reg pc}')
            assert int(stopped.split()[-1], 16) == address, stopped
        finally:
            ocd.command(f'rbp {address}; resume')
        fixture.wait(lambda: fixture.words()[3] == sequence)
        assert ocd.read(fixture.symbols['swd_scratch'], 1024) == [0x5a5a5a5a] * 1024
        return {'rom_address': hex(address), 'stopped_pc': stopped}
    test('swd_hardware_breakpoint', hardware_breakpoint)

    def debug_controls():
        tick = fixture.symbols['work_tick']
        ocd.command('halt')
        # Cortex-M0+ hardware FPB cannot break at SRAM addresses; use BKPT.
        ocd.command(f'bp 0x{tick:x} 2')
        try:
            ocd.command('resume; wait_halt 2000')
            breakpoint_pc = ocd.command('capture {reg pc}')
        finally:
            ocd.command(f'rbp 0x{tick:x}')
        ocd.command('step')
        step_pc = ocd.command('capture {reg pc}')
        if breakpoint_pc == step_pc:
            ocd.command('resume')
            raise AssertionError('single step did not advance PC')
        watched = fixture.mailbox + 24
        ocd.command(f'wp 0x{watched:x} 4 w')
        try:
            ocd.command('resume; wait_halt 2000')
            watchpoint_pc = ocd.command('capture {reg pc}')
        finally:
            ocd.command(f'rwp 0x{watched:x}; resume')
        return {'breakpoint_pc': breakpoint_pc, 'step_pc': step_pc, 'watchpoint_pc': watchpoint_pc}
    if not test('swd_software_breakpoint_step_watchpoint', debug_controls):
        ocd.command('halt; resume')

    def capture_completion_count():
        fixture.command(1, 0)
        for _ in range(200):
            reply = Probe._ok(probe.command(Command.CAPTURE, struct.pack('<II', 256, 100000)), 'capture')
            _, count, elapsed = struct.unpack('<III', reply)
            assert count == 256 and elapsed > 0, f'completion reported {count} samples in {elapsed} us'
        return {'captures': 200, 'samples_each': 256}
    test('capture_completion_count', capture_completion_count)

    for pin in DIRECT_PINS:
        def gpio_pin(pin=pin):
            for pattern in (1 << pin, DIRECT_MASK ^ (1 << pin)):
                fixture.command(1, pattern)
                observed = fixture.gpio() & DIRECT_MASK
                assert observed == pattern, f'GPIO_READ expected {pattern:#x}, got {observed:#x}'
                capture = probe.capture(256, 100000)
                assert all(sample & DIRECT_MASK == pattern for sample in capture.samples), 'capture pin mismatch'
            return {'pin': pin, 'walking_one_and_zero': True}
        test(f'gpio_work_gp{pin}', gpio_pin)

    def dynamic_capture():
        fixture.command(2)
        capture = probe.capture(32768, 1000000)
        samples = [v & DIRECT_MASK for v in capture.samples]
        seen = set(samples)
        assert all(1 << pin in seen for pin in DIRECT_PINS), f'missing pins, seen={seen}'
        valid = sum(v != 0 and (v & (v - 1)) == 0 for v in samples)
        assert valid / len(samples) > .99, f'{len(samples) - valid} invalid patterns'
        fixture.command(1, 0)
        return {'samples': len(samples), 'actual_hz': round(capture.actual_frequency),
                'valid_one_hot_samples': valid, 'seen_patterns': sorted(seen)}
    test('gpio_dynamic_capture', dynamic_capture)

    for baud in (115200, 1000000):
        def uart(baud=baud):
            fixture.command(3, baud)
            debug_baud = probe.uart_config(0, baud)
            work_baud = fixture.words()[12]
            transmitted = bytes((i * 37 + (i >> 8)) & 255 for i in range(4096))
            received = bytearray()
            for index in range(0, len(transmitted), 256):
                block = transmitted[index:index + 256]
                probe.uart_write(0, block)
                deadline = time.monotonic() + 2
                while len(received) < index + len(block) and time.monotonic() < deadline:
                    received.extend(probe.uart_read(0))
                    if len(received) < index + len(block):
                        time.sleep(.003)
                assert len(received) == index + len(block), f'received {len(received)} bytes'
            assert bytes(received) == transmitted, 'echo data mismatch'
            status = asdict(probe.uart_status(0))
            assert status['rx_overruns'] == 0
            return {'bytes_each_direction': len(transmitted), 'debug_baud': debug_baud,
                    'work_baud': work_baud, 'status': status}
        test(f'uart_echo_{baud}', uart)

    for mode in range(4):
        for rate in (100000, 1000000):
            def spi(mode=mode, rate=rate):
                actual = probe.spi_config(rate, mode)
                initial = fixture.words()
                payload = spi_test_pattern()
                for _ in range(5):
                    fixture.command(4, mode)
                    blocks = fixture.words()[9]
                    received = probe.spi_transfer(payload)
                    errors = sum(a != (b ^ 0xa5) for a, b in zip(received, payload))
                    assert len(received) == len(payload) and errors == 0, f'MISO errors={errors}, bytes={len(received)}'
                    fixture.wait(lambda: fixture.words()[9] == blocks + 1)
                    assert fixture.words()[10] == initial[10], 'Work MOSI data errors'
                return {'mode': mode, 'actual_hz': actual, 'blocks': 5,
                        'bytes_each_direction': len(payload) * 5}
            test(f'spi_mode{mode}_{rate}', spi)

    def bootsel_wire():
        # Float CS as the ROM does; SRAM execution avoids fetching flash.
        pin = 0xd0000008  # RP2040 SIO GPIO_HI_IN, QSPI SS_N is bit 1.
        control = 0x4001800c  # RP2040 IO_QSPI GPIO_QSPI_SS_CTRL.
        csn = 1 << 1
        original = ocd.read(control)[0]
        ocd.write(control, [(original & ~0x3000) | 0x2000])
        try:
            before = ocd.read(pin)[0] & csn
            probe._ok(probe.command(Command.BOOTSEL_SET, b'\x01'), 'assert BOOTSEL')
            asserted = ocd.read(pin)[0] & csn
            probe._ok(probe.command(Command.BOOTSEL_SET, b'\x00'), 'release BOOTSEL')
            released = ocd.read(pin)[0] & csn
        finally:
            probe._ok(probe.command(Command.BOOTSEL_SET, b'\x00'), 'release BOOTSEL')
            ocd.write(control, [original])
        assert before == csn and asserted == 0 and released == csn, (before, asserted, released)
        return {'before': before, 'asserted': asserted, 'released': released}
    test('bootsel_gate_to_work_qspi_csn', bootsel_wire)

    def powered_status():
        status = fixture.status()
        assert status['power_on'] and status['power_good'] and not status['power_fault'], status
        assert 3000 < status['work_3v3_mv'] < 3600, status
        return status
    test('powered_status', powered_status)

    def measurement():
        value = asdict(Measurement.from_payload(probe.command(Command.MEASURE)))
        assert 4000 < value['bus_mv'] < 5500 and 1 < value['current_ma'] < 350, value
        return value
    test('ina237_loaded_measurement', measurement)
    raise SystemExit(0 if all(t['passed'] for t in results['tests']) else 1)


if __name__ == '__main__':
    main()
